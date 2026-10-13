//! Responsive pane geometry shared by cell and native presentations.
//!
//! A tree describes intent; [`Node::layout`] produces an immutable frame for
//! drawing, hit testing, focus and inspection. Solving never rewrites user
//! visibility or preferred sizes. All lengths use the caller's chosen unit
//! (cells or physical pixels); a tree must use one unit consistently.
//!
//! Rows can wrap at their children's minimum widths. When minimums still do
//! not fit, lower-priority siblings collapse, then hide, according to their
//! policy. Higher priorities receive their preferred size first; weighted
//! growth distributes the remainder without exceeding maximums. Mandatory
//! content that cannot fit is explicitly reported as clipped.

use std::cell::Cell;
use std::collections::HashSet;
use std::hash::Hash;

use crate::ratatui::layout::Rect;
use serde::Serialize;

pub use crate::dock::Axis;

/// Bounds along one axis. Constructed values are normalized during layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Size {
    pub min: u16,
    pub preferred: u16,
    pub max: u16,
    /// Weight for space remaining after preferred sizes; zero keeps the preference.
    pub grow: u16,
}
impl Size {
    pub const fn fixed(size: u16) -> Self {
        Self {
            min: size,
            preferred: size,
            max: size,
            grow: 0,
        }
    }
    pub const fn bounded(min: u16, preferred: u16, max: u16) -> Self {
        Self {
            min,
            preferred,
            max,
            grow: 0,
        }
    }
    pub const fn fill(min: u16) -> Self {
        Self {
            min,
            preferred: min,
            max: u16::MAX,
            grow: 1,
        }
    }
    pub const fn grow(mut self, weight: u16) -> Self {
        self.grow = weight;
        self
    }
    fn normalized(self) -> Self {
        let max = self.max.max(self.min);
        Self {
            preferred: self.preferred.clamp(self.min, max),
            max,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Wrap {
    #[default]
    Never,
    /// Keep siblings together while their minimum extents fit; otherwise wrap.
    AtMinimum,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    #[default]
    Expanded,
    /// A user collapse retains the pane's compact geometry.
    Collapsed,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Overflow {
    #[default]
    Keep,
    Hide,
    /// Use compact geometry before hiding; neither action changes user intent.
    CollapseThenHide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusPolicy {
    Never,
    /// Can receive explicit focus, but is excluded from Tab traversal.
    Direct,
    #[default]
    Cycle,
}

/// Global viewport thresholds, independent of a pane's eventual allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ViewportRule {
    pub min_width: u16,
    pub min_height: u16,
    pub max_width: Option<u16>,
    pub max_height: Option<u16>,
}
impl ViewportRule {
    fn matches(self, area: Rect) -> bool {
        area.width >= self.min_width
            && area.height >= self.min_height
            && self.max_width.is_none_or(|max| area.width <= max)
            && self.max_height.is_none_or(|max| area.height <= max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Requested,
    User,
    ViewportRule,
    InsufficientSpace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct State {
    pub visibility: Visibility,
    pub reason: Reason,
    /// Mandatory content can receive bounded geometry smaller than its minimum.
    pub clipped: bool,
}

#[derive(Clone, Debug)]
enum Content<P> {
    Pane(P),
    Group {
        axis: Axis,
        gap: u16,
        wrap: Wrap,
        children: Vec<Node<P>>,
    },
}

/// A pane or a nested row/column. Builder methods apply to either kind.
#[derive(Clone, Debug)]
pub struct Node<P> {
    content: Content<P>,
    width: Option<Size>,
    height: Option<Size>,
    priority: u16,
    visibility: Visibility,
    rule: ViewportRule,
    overflow: Overflow,
    compact: [Size; 2],
    focus: FocusPolicy,
}
impl<P> Node<P> {
    pub fn pane(id: P) -> Self {
        Self::new(Content::Pane(id))
    }
    pub fn row(children: Vec<Self>) -> Self {
        Self::group(Axis::Horizontal, children)
    }
    pub fn column(children: Vec<Self>) -> Self {
        Self::group(Axis::Vertical, children)
    }
    fn group(axis: Axis, children: Vec<Self>) -> Self {
        Self::new(Content::Group {
            axis,
            gap: 0,
            wrap: Wrap::Never,
            children,
        })
    }
    fn new(content: Content<P>) -> Self {
        Self {
            content,
            width: None,
            height: None,
            priority: 0,
            visibility: Visibility::Expanded,
            rule: ViewportRule::default(),
            overflow: Overflow::Keep,
            compact: [Size::fill(1), Size::fixed(1)],
            focus: FocusPolicy::Cycle,
        }
    }
    pub fn width(mut self, size: Size) -> Self {
        self.width = Some(size);
        self
    }
    pub fn height(mut self, size: Size) -> Self {
        self.height = Some(size);
        self
    }
    pub fn priority(mut self, priority: u16) -> Self {
        self.priority = priority;
        self
    }
    pub fn visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }
    pub fn when(mut self, rule: ViewportRule) -> Self {
        self.rule = rule;
        self
    }
    pub fn overflow(mut self, policy: Overflow) -> Self {
        self.overflow = policy;
        self
    }
    pub fn compact(mut self, width: Size, height: Size) -> Self {
        self.compact = [width, height];
        self
    }
    pub fn focus(mut self, policy: FocusPolicy) -> Self {
        self.focus = policy;
        self
    }
    pub fn gap(mut self, gap: u16) -> Self {
        if let Content::Group { gap: value, .. } = &mut self.content {
            *value = gap;
        }
        self
    }
    pub fn wrap(mut self, wrap: Wrap) -> Self {
        if let Content::Group { wrap: value, .. } = &mut self.content {
            *value = wrap;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Placement<P> {
    pub id: P,
    #[serde(serialize_with = "serialize_rect")]
    pub area: Option<Rect>,
    pub state: State,
    pub focus: FocusPolicy,
}
fn serialize_rect<S: serde::Serializer>(
    area: &Option<Rect>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Bounds {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    }
    area.map(|r| Bounds {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    })
    .serialize(serializer)
}

/// Inspectable geometry in stable tree order, including every hidden pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Layout<P> {
    pub panes: Vec<Placement<P>>,
    /// False when mandatory content could not meet its declared minimum.
    pub fits: bool,
}
impl<P: Copy + Eq> Layout<P> {
    pub fn pane(&self, id: P) -> Option<&Placement<P>> {
        self.panes.iter().find(|p| p.id == id)
    }
    pub fn rect_of(&self, id: P) -> Option<Rect> {
        self.pane(id).and_then(|p| p.area)
    }
    pub fn hit(&self, x: u16, y: u16) -> Option<P> {
        self.panes
            .iter()
            .find(|p| p.area.is_some_and(|r| r.contains((x, y).into())))
            .map(|p| p.id)
    }
    pub fn focus_order(&self) -> impl Iterator<Item = P> + '_ {
        self.panes
            .iter()
            .filter(|p| p.area.is_some() && p.focus == FocusPolicy::Cycle)
            .map(|p| p.id)
    }
    pub fn can_focus(&self, id: P) -> bool {
        self.pane(id)
            .is_some_and(|p| p.area.is_some() && p.focus != FocusPolicy::Never)
    }
}

/// Reject ambiguous identities and unbounded recursion before solving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    DuplicateId,
    TooManyNodes,
    TooDeep,
    CollapsedGroup,
}
impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DuplicateId => "duplicate pane id",
            Self::TooManyNodes => "pane tree exceeds 4096 nodes",
            Self::TooDeep => "pane tree exceeds 64 levels",
            Self::CollapsedGroup => "a group cannot collapse: use a pane for its compact header",
        })
    }
}
impl std::error::Error for LayoutError {}

#[derive(Clone, Copy)]
struct Measurement {
    dimensions: (u16, u16),
    sizes: [Size; 2],
}

struct Frame<'a, P> {
    node: &'a Node<P>,
    children: Vec<Self>,
    state: State,
    focus: FocusPolicy,
    measured: Cell<Option<Measurement>>,
}
impl<P: Copy + Eq + Hash> Node<P> {
    pub fn layout(&self, area: Rect) -> Result<Layout<P>, LayoutError> {
        let mut seen = HashSet::new();
        let mut count = 0;
        let mut frame = Frame::prepare(
            self,
            area,
            None,
            FocusPolicy::Cycle,
            0,
            &mut count,
            &mut seen,
        )?;
        // Rect fields can be constructed directly. Bound their far edges too.
        let area = Rect::new(
            area.x,
            area.y,
            area.width.min(u16::MAX - area.x),
            area.height.min(u16::MAX - area.y),
        );
        let mut layout = Layout {
            panes: Vec::with_capacity(seen.len()),
            fits: true,
        };
        let sizes = frame.sizes(area);
        let area = Rect::new(
            area.x,
            area.y,
            area.width.min(sizes[0].max),
            area.height.min(sizes[1].max),
        );
        frame.place(area, &mut layout);
        Ok(layout)
    }
}
impl<'a, P: Copy + Eq + Hash> Frame<'a, P> {
    fn prepare(
        node: &'a Node<P>,
        viewport: Rect,
        inherited: Option<State>,
        parent_focus: FocusPolicy,
        depth: usize,
        count: &mut usize,
        seen: &mut HashSet<P>,
    ) -> Result<Self, LayoutError> {
        *count += 1;
        if *count > 4096 {
            return Err(LayoutError::TooManyNodes);
        }
        if depth > 64 {
            return Err(LayoutError::TooDeep);
        }
        if matches!(node.content, Content::Group { .. })
            && (node.visibility == Visibility::Collapsed
                || node.overflow == Overflow::CollapseThenHide)
        {
            return Err(LayoutError::CollapsedGroup);
        }
        let state = if node.visibility == Visibility::Hidden {
            State {
                visibility: Visibility::Hidden,
                reason: Reason::User,
                clipped: false,
            }
        } else if let Some(state) = inherited {
            state
        } else if !node.rule.matches(viewport) {
            State {
                visibility: Visibility::Hidden,
                reason: Reason::ViewportRule,
                clipped: false,
            }
        } else {
            State {
                visibility: node.visibility,
                reason: if node.visibility == Visibility::Collapsed {
                    Reason::User
                } else {
                    Reason::Requested
                },
                clipped: false,
            }
        };
        let focus = match (parent_focus, node.focus) {
            (FocusPolicy::Never, _) | (_, FocusPolicy::Never) => FocusPolicy::Never,
            (FocusPolicy::Direct, _) | (_, FocusPolicy::Direct) => FocusPolicy::Direct,
            _ => FocusPolicy::Cycle,
        };
        let children = match &node.content {
            Content::Pane(id) => {
                if !seen.insert(*id) {
                    return Err(LayoutError::DuplicateId);
                }
                Vec::new()
            }
            Content::Group { children, .. } => children
                .iter()
                .map(|child| {
                    Self::prepare(
                        child,
                        viewport,
                        (state.visibility == Visibility::Hidden).then_some(state),
                        focus,
                        depth + 1,
                        count,
                        seen,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(Self {
            node,
            children,
            state,
            focus,
            measured: Cell::new(None),
        })
    }
    fn hidden(&self) -> bool {
        self.state.visibility == Visibility::Hidden
            || (!self.children.is_empty() && self.children.iter().all(Self::hidden))
    }
    fn sizes(&self, area: Rect) -> [Size; 2] {
        if let Some(measured) = self.measured.get() {
            if measured.dimensions == (area.width, area.height) {
                return measured.sizes;
            }
        }
        let sizes = self.measure(area);
        self.measured.set(Some(Measurement {
            dimensions: (area.width, area.height),
            sizes,
        }));
        sizes
    }
    fn measure(&self, area: Rect) -> [Size; 2] {
        if self.hidden() {
            return [Size::fixed(0); 2];
        }
        if self.state.visibility == Visibility::Collapsed {
            return self.node.compact.map(Size::normalized);
        }
        let defaults = match &self.node.content {
            Content::Pane(_) => [Size::fill(1); 2],
            Content::Group {
                axis, gap, wrap, ..
            } => {
                let lines = self.lines(area, *axis, *gap, *wrap);
                let mut along_pref = 0u32;
                let mut cross_pref = 0u32;
                for line in &lines {
                    let spacing = u32::from(*gap) * line.len().saturating_sub(1) as u32;
                    let mut apref = spacing;
                    let mut cpref = 0;
                    for &index in line {
                        let sizes = self.children[index].sizes(area);
                        let a = sizes[axis_index(*axis)];
                        let c = sizes[1 - axis_index(*axis)];
                        apref += u32::from(a.preferred);
                        cpref = cpref.max(u32::from(c.preferred));
                    }
                    along_pref = along_pref.max(apref);
                    cross_pref += cpref;
                }
                let spacing = u32::from(*gap) * lines.len().saturating_sub(1) as u32;
                // Containers adapt their children within the allocation. Their
                // own floor is explicit, rather than forcing all descendants'
                // expanded minimums onto an ancestor before wrapping/collapse.
                let along = Size::bounded(0, cap(along_pref), u16::MAX).grow(1);
                let cross = Size::bounded(0, cap(cross_pref + spacing), u16::MAX).grow(1);
                if *axis == Axis::Horizontal {
                    [along, cross]
                } else {
                    [cross, along]
                }
            }
        };
        [
            self.node.width.unwrap_or(defaults[0]).normalized(),
            self.node.height.unwrap_or(defaults[1]).normalized(),
        ]
    }
    fn lines(&self, area: Rect, axis: Axis, gap: u16, wrap: Wrap) -> Vec<Vec<usize>> {
        let mut lines = Vec::<Vec<usize>>::new();
        let mut used = 0u32;
        for (index, child) in self
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| !child.hidden())
        {
            let size = child.sizes(area)[axis_index(axis)];
            let next = used + u32::from(gap) + u32::from(size.min);
            if wrap == Wrap::AtMinimum
                && lines.last().is_some_and(|line| !line.is_empty())
                && next > u32::from(extent(area, axis))
            {
                lines.push(Vec::new());
                used = 0;
            }
            if lines.is_empty() {
                lines.push(Vec::new());
            }
            let line = lines.last_mut().unwrap();
            if !line.is_empty() {
                used += u32::from(gap);
            }
            used += u32::from(size.min);
            line.push(index);
        }
        lines
    }
    fn shortage(&self, area: Rect, axis: Axis, gap: u16, wrap: Wrap) -> u32 {
        let lines = self.lines(area, axis, gap, wrap);
        let mut cross_total = u32::from(gap) * lines.len().saturating_sub(1) as u32;
        let mut shortage = 0;
        for line in &lines {
            let mut main_total = u32::from(gap) * line.len().saturating_sub(1) as u32;
            let mut cross = 0;
            for &index in line {
                let sizes = self.children[index].sizes(area);
                main_total += u32::from(sizes[axis_index(axis)].min);
                cross = cross.max(u32::from(sizes[1 - axis_index(axis)].min));
            }
            shortage += main_total.saturating_sub(u32::from(extent(area, axis)));
            cross_total += cross;
        }
        shortage + cross_total.saturating_sub(u32::from(extent(area, other(axis))))
    }
    fn degrade(&mut self, area: Rect, axis: Axis, gap: u16, wrap: Wrap) -> bool {
        let before = self.shortage(area, axis, gap, wrap);
        // Lower priority degrades first; at equal priority later siblings yield.
        let mut candidates: Vec<_> = self
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| !child.hidden() && child.node.overflow != Overflow::Keep)
            .map(|(i, child)| (child.node.priority, std::cmp::Reverse(i), i))
            .collect();
        candidates.sort_unstable();
        for (_, _, index) in candidates {
            let previous = self.children[index].state;
            let collapse = previous.visibility == Visibility::Expanded
                && self.children[index].node.overflow == Overflow::CollapseThenHide;
            for visibility in [Visibility::Collapsed, Visibility::Hidden] {
                if visibility == Visibility::Collapsed && !collapse {
                    continue;
                }
                self.children[index].state = State {
                    visibility,
                    reason: Reason::InsufficientSpace,
                    clipped: false,
                };
                self.children[index].measured.set(None);
                if self.shortage(area, axis, gap, wrap) < before {
                    self.measured.set(None);
                    return true;
                }
            }
            self.children[index].state = previous;
            self.children[index].measured.set(None);
        }
        false
    }
    fn place(&mut self, area: Rect, out: &mut Layout<P>) {
        if self.hidden() {
            self.emit_hidden(out, self.state);
            return;
        }
        if let Content::Pane(id) = self.node.content {
            let sizes = self.sizes(area);
            let clipped = area.width < sizes[0].min || area.height < sizes[1].min;
            let visible = area.width > 0 && area.height > 0;
            out.fits &= !clipped;
            out.panes.push(Placement {
                id,
                area: visible.then_some(area),
                state: State {
                    clipped,
                    visibility: if visible {
                        self.state.visibility
                    } else {
                        Visibility::Hidden
                    },
                    reason: if clipped {
                        Reason::InsufficientSpace
                    } else {
                        self.state.reason
                    },
                },
                focus: self.focus,
            });
            return;
        }
        let Content::Group {
            axis, gap, wrap, ..
        } = self.node.content
        else {
            unreachable!()
        };
        let sizes = self.sizes(area);
        if area.width < sizes[0].min || area.height < sizes[1].min {
            out.fits = false;
        }
        while self.shortage(area, axis, gap, wrap) > 0 && self.degrade(area, axis, gap, wrap) {}
        let lines = self.lines(area, axis, gap, wrap);
        let cross_sizes: Vec<_> = lines
            .iter()
            .map(|line| {
                let mut min = 0;
                let mut pref = 0;
                let mut max = 0;
                let mut grow = 0;
                let mut priority = 0;
                for &index in line {
                    let c = self.children[index].sizes(area)[1 - axis_index(axis)];
                    min = min.max(c.min);
                    pref = pref.max(c.preferred);
                    max = max.max(c.max);
                    grow = grow.max(c.grow);
                    priority = priority.max(self.children[index].node.priority);
                }
                (Size::bounded(min, pref, max).grow(grow), priority)
            })
            .collect();
        let cross_gap = actual_gap(gap, lines.len(), extent(area, other(axis)));
        let cross_extents = allocate(
            &cross_sizes,
            extent(area, other(axis))
                .saturating_sub(cross_gap.saturating_mul(lines.len().saturating_sub(1) as u16)),
        );
        let mut rects = vec![None; self.children.len()];
        let mut cross_pos = origin(area, other(axis));
        for (line, cross_extent) in lines.iter().zip(cross_extents) {
            let gap = actual_gap(gap, line.len(), extent(area, axis));
            let available = extent(area, axis)
                .saturating_sub(gap.saturating_mul(line.len().saturating_sub(1) as u16));
            let sizes: Vec<_> = line
                .iter()
                .map(|&i| {
                    (
                        self.children[i].sizes(area)[axis_index(axis)],
                        self.children[i].node.priority,
                    )
                })
                .collect();
            let lengths = allocate(&sizes, available);
            let mut pos = origin(area, axis);
            for (&index, length) in line.iter().zip(lengths) {
                let cross_size = self.children[index].sizes(area)[1 - axis_index(axis)];
                let cross = cross_extent.min(cross_size.max);
                rects[index] = Some(if axis == Axis::Horizontal {
                    Rect::new(pos, cross_pos, length, cross)
                } else {
                    Rect::new(cross_pos, pos, cross, length)
                });
                pos = pos.saturating_add(length).saturating_add(gap);
            }
            cross_pos = cross_pos
                .saturating_add(cross_extent)
                .saturating_add(cross_gap);
        }
        // Retain tree order even when rows wrap or a child disappears.
        for (child, rect) in self.children.iter_mut().zip(rects) {
            if let Some(rect) = rect {
                child.place(rect, out);
            } else {
                child.emit_hidden(out, child.state);
            }
        }
    }
    fn emit_hidden(&self, out: &mut Layout<P>, inherited: State) {
        match self.node.content {
            Content::Pane(id) => out.panes.push(Placement {
                id,
                area: None,
                state: State {
                    visibility: Visibility::Hidden,
                    ..inherited
                },
                focus: self.focus,
            }),
            Content::Group { .. } => {
                for child in &self.children {
                    child.emit_hidden(
                        out,
                        if child.state.reason == Reason::User
                            && child.state.visibility == Visibility::Hidden
                        {
                            child.state
                        } else if self.state.visibility == Visibility::Hidden {
                            inherited
                        } else {
                            child.state
                        },
                    );
                }
            }
        }
    }
}
fn cap(n: u32) -> u16 {
    n.min(u32::from(u16::MAX)) as u16
}
fn axis_index(axis: Axis) -> usize {
    if axis == Axis::Horizontal {
        0
    } else {
        1
    }
}
fn other(axis: Axis) -> Axis {
    if axis == Axis::Horizontal {
        Axis::Vertical
    } else {
        Axis::Horizontal
    }
}
fn extent(area: Rect, axis: Axis) -> u16 {
    if axis == Axis::Horizontal {
        area.width
    } else {
        area.height
    }
}
fn origin(area: Rect, axis: Axis) -> u16 {
    if axis == Axis::Horizontal {
        area.x
    } else {
        area.y
    }
}
fn actual_gap(gap: u16, count: usize, available: u16) -> u16 {
    if count < 2 {
        0
    } else {
        gap.min(available / (count - 1) as u16)
    }
}

/// Priority preferences followed by capped, weighted growth. Remainders go to
/// later siblings deterministically; all arithmetic is wider than layout units.
fn allocate(sizes: &[(Size, u16)], available: u16) -> Vec<u16> {
    let mut out: Vec<u16> = sizes.iter().map(|(s, _)| s.min).collect();
    let sum: u32 = out.iter().map(|&v| u32::from(v)).sum();
    let mut order: Vec<_> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(sizes[i].1), i));
    if sum > u32::from(available) {
        let mut left = available;
        for i in order {
            out[i] = out[i].min(left);
            left -= out[i];
        }
        return out;
    }
    let mut left = available - sum as u16;
    for &i in &order {
        let extra = sizes[i].0.preferred.saturating_sub(out[i]).min(left);
        out[i] += extra;
        left -= extra;
    }
    while left > 0 {
        let candidates: Vec<_> = (0..sizes.len())
            .filter(|&i| sizes[i].0.grow > 0 && out[i] < sizes[i].0.max)
            .collect();
        let weight: u64 = candidates.iter().map(|&i| u64::from(sizes[i].0.grow)).sum();
        if weight == 0 {
            break;
        }
        let budget = left;
        let mut assigned = 0;
        for (n, &i) in candidates.iter().enumerate() {
            let share = if n + 1 == candidates.len() {
                budget - assigned
            } else {
                (u64::from(budget) * u64::from(sizes[i].0.grow) / weight) as u16
            };
            assigned += share;
            let extra = share.min(sizes[i].0.max - out[i]).min(left);
            out[i] += extra;
            left -= extra;
        }
    }
    out
}

/// Focus intent survives responsive hiding. A user step replaces that intent;
/// growing the viewport then restores only the most recently requested pane.
#[derive(Clone, Debug, Serialize)]
pub struct Focus<P> {
    requested: Option<P>,
    active: Option<P>,
    scopes: Vec<(Vec<P>, Option<P>)>,
}
impl<P: Copy + Eq> Focus<P> {
    pub fn new(initial: Option<P>) -> Self {
        Self {
            requested: initial,
            active: initial,
            scopes: Vec::new(),
        }
    }
    pub fn requested(&self) -> Option<P> {
        self.requested
    }
    pub fn scope_depth(&self) -> usize {
        self.scopes.len()
    }
    pub fn active(&self) -> Option<P> {
        self.active
    }
    pub fn request(&mut self, id: P) {
        if self
            .scopes
            .last()
            .is_none_or(|(scope, _)| scope.contains(&id))
        {
            if self.scopes.is_empty() {
                self.requested = Some(id);
            }
            self.active = Some(id);
        }
    }
    pub fn reconcile(&mut self, layout: &Layout<P>) -> Option<P> {
        if self.scopes.is_empty() {
            self.active = self
                .requested
                .filter(|&id| layout.can_focus(id))
                .or_else(|| self.active.filter(|&id| layout.can_focus(id)))
                .or_else(|| layout.focus_order().next());
        }
        self.active
    }
    pub fn step(&mut self, layout: &Layout<P>, forward: bool) -> Option<P> {
        let order: Vec<_> = if let Some((scope, _)) = self.scopes.last() {
            scope.clone()
        } else {
            layout.focus_order().collect()
        };
        self.step_in(&order, forward)
    }
    pub fn step_in(&mut self, order: &[P], forward: bool) -> Option<P> {
        let scope = self.scopes.last().map(|(scope, _)| scope.clone());
        let order = scope.as_deref().unwrap_or(order);
        if order.is_empty() {
            self.active = None;
            return None;
        }
        let index = self
            .active
            .and_then(|id| order.iter().position(|&p| p == id));
        let next = match (index, forward) {
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
            (None, true) => 0,
            (None, false) => order.len() - 1,
        };
        self.active = Some(order[next]);
        if self.scopes.is_empty() {
            self.requested = self.active;
        }
        self.active
    }
    /// Push an exclusive modal focus scope; empty scopes are rejected.
    pub fn push_scope(&mut self, order: Vec<P>) -> bool {
        let Some(&first) = order.first() else {
            return false;
        };
        self.scopes.push((order, self.active));
        self.active = Some(first);
        true
    }
    pub fn pop_scope(&mut self, layout: &Layout<P>) -> Option<P> {
        if let Some((_, previous)) = self.scopes.pop() {
            self.active = previous;
        }
        self.reconcile(layout)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    Leading,
    Trailing,
}
/// A stable press origin independent of repaints and coordinate precision.
#[derive(Clone, Copy, Debug)]
pub struct ResizeCapture {
    origin: i32,
    initial: u16,
    min: u16,
    max: u16,
    step: u16,
    edge: ResizeEdge,
}
impl ResizeCapture {
    pub fn new(origin: i32, initial: u16, min: u16, max: u16, edge: ResizeEdge) -> Self {
        Self {
            origin,
            initial,
            min,
            max: max.max(min),
            step: 1,
            edge,
        }
    }
    pub fn step(mut self, units: u16) -> Self {
        self.step = units.max(1);
        self
    }
    pub fn size_at(self, coordinate: i32) -> u16 {
        let delta = (i64::from(coordinate) - i64::from(self.origin)) / i64::from(self.step);
        let sign = if self.edge == ResizeEdge::Leading {
            -1
        } else {
            1
        };
        (i64::from(self.initial) + delta * sign).clamp(i64::from(self.min), i64::from(self.max))
            as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn nested_flow_wraps_before_losing_content_and_preserves_hit_geometry() {
        let row = Node::row(
            (0..3)
                .map(|id| {
                    Node::pane(id)
                        .width(Size::bounded(40, 40, 80).grow(1))
                        .height(Size::fixed(5))
                })
                .collect(),
        )
        .gap(2)
        .wrap(Wrap::AtMinimum);
        let tree = Node::column(vec![row, Node::pane(3).height(Size::fixed(1))]);
        let wide = tree.layout(Rect::new(5, 7, 124, 11)).unwrap();
        assert!(wide.fits);
        assert_eq!(wide.rect_of(0).unwrap().y, wide.rect_of(2).unwrap().y);
        let narrow = tree.layout(Rect::new(5, 7, 82, 13)).unwrap();
        assert!(narrow.fits);
        assert_eq!(narrow.rect_of(0).unwrap(), Rect::new(5, 7, 40, 5));
        assert_eq!(narrow.rect_of(1).unwrap(), Rect::new(47, 7, 40, 5));
        assert_eq!(narrow.rect_of(2).unwrap(), Rect::new(5, 14, 80, 5));
        assert_eq!(narrow.hit(5, 14), Some(2));
        assert_eq!(narrow.hit(46, 8), None, "gaps are not pane content");
    }

    #[test]
    fn preferences_priority_growth_and_maximums_have_separate_roles() {
        let tree = Node::column(vec![
            Node::pane(0).height(Size::fill(4)),
            Node::pane(1).height(Size::bounded(2, 8, 8)).priority(1),
            Node::pane(2).height(Size::bounded(2, 6, 6)).priority(2),
        ]);
        let small = tree.layout(Rect::new(0, 0, 20, 14)).unwrap();
        assert_eq!(small.rect_of(0).unwrap().height, 4);
        assert_eq!(small.rect_of(1).unwrap().height, 4);
        assert_eq!(small.rect_of(2).unwrap().height, 6);
        let large = tree.layout(Rect::new(0, 0, 20, 30)).unwrap();
        assert_eq!(large.rect_of(0).unwrap().height, 16);
        assert_eq!(large.rect_of(1).unwrap().height, 8);
        assert_eq!(large.rect_of(2).unwrap().height, 6);
        let capped = Node::row(vec![
            Node::pane(0).width(Size::bounded(2, 2, 4).grow(1)),
            Node::pane(1).width(Size::bounded(2, 2, 30).grow(2)),
        ])
        .layout(Rect::new(0, 0, 20, 4))
        .unwrap();
        assert_eq!(capped.rect_of(0).unwrap().width, 4);
        assert_eq!(capped.rect_of(1).unwrap().width, 16);
    }

    #[test]
    fn responsive_collapse_hide_and_user_intent_are_distinct_and_recover() {
        let tree = Node::column(vec![
            Node::pane(0).height(Size::fill(5)).priority(10),
            Node::pane(1)
                .height(Size::fixed(8))
                .overflow(Overflow::CollapseThenHide)
                .compact(Size::fill(1), Size::fixed(2)),
            Node::pane(2).visibility(Visibility::Hidden),
            Node::pane(3)
                .when(ViewportRule {
                    min_width: 80,
                    ..ViewportRule::default()
                })
                .height(Size::fixed(1)),
        ])
        .gap(1);
        let collapsed = tree.layout(Rect::new(0, 0, 40, 8)).unwrap();
        assert!(collapsed.fits);
        assert_eq!(collapsed.rect_of(1).unwrap(), Rect::new(0, 6, 40, 2));
        assert_eq!(
            collapsed.pane(1).unwrap().state.reason,
            Reason::InsufficientSpace
        );
        assert_eq!(
            collapsed.pane(1).unwrap().state.visibility,
            Visibility::Collapsed
        );
        let hidden = tree.layout(Rect::new(0, 0, 40, 5)).unwrap();
        assert_eq!(hidden.rect_of(1), None);
        assert_eq!(
            hidden.pane(1).unwrap().state.reason,
            Reason::InsufficientSpace
        );
        assert_eq!(hidden.pane(2).unwrap().state.reason, Reason::User);
        assert_eq!(hidden.pane(3).unwrap().state.reason, Reason::ViewportRule);
        let restored = tree.layout(Rect::new(0, 0, 80, 20)).unwrap();
        assert_eq!(
            restored.pane(1).unwrap().state.visibility,
            Visibility::Expanded
        );
        assert_eq!(restored.rect_of(1).unwrap().height, 8);
        assert_eq!(restored.rect_of(2), None);
        assert!(restored.rect_of(3).is_some());
    }

    #[test]
    fn rules_apply_to_user_collapsed_panes_and_hidden_ancestors_keep_reasons() {
        let tree = Node::column(vec![
            Node::column(vec![
                Node::pane(0).visibility(Visibility::Hidden),
                Node::pane(1),
            ])
            .when(ViewportRule {
                min_width: 90,
                ..ViewportRule::default()
            }),
            Node::pane(2)
                .visibility(Visibility::Collapsed)
                .when(ViewportRule {
                    min_width: 90,
                    ..ViewportRule::default()
                }),
        ]);
        let layout = tree.layout(Rect::new(0, 0, 40, 20)).unwrap();
        assert_eq!(layout.pane(0).unwrap().state.reason, Reason::User);
        assert_eq!(layout.pane(1).unwrap().state.reason, Reason::ViewportRule);
        assert_eq!(layout.pane(2).unwrap().state.reason, Reason::ViewportRule);
        assert!(layout.panes.iter().all(|p| p.area.is_none()));
    }

    #[test]
    fn mandatory_overflow_does_not_hide_unrelated_fitting_content() {
        let tree = Node::column(vec![
            Node::pane(0).width(Size::fixed(100)).height(Size::fixed(3)),
            Node::pane(1)
                .height(Size::fixed(2))
                .overflow(Overflow::Hide),
        ]);
        let layout = tree.layout(Rect::new(0, 0, 50, 10)).unwrap();
        assert!(!layout.fits);
        assert!(layout.pane(0).unwrap().state.clipped);
        assert!(layout.rect_of(1).is_some());
        let capped = Node::pane(0)
            .width(Size::fixed(10))
            .height(Size::fixed(3))
            .layout(Rect::new(0, 0, 100, 40))
            .unwrap();
        assert_eq!(capped.rect_of(0).unwrap(), Rect::new(0, 0, 10, 3));
    }

    #[test]
    fn focus_restores_after_resize_but_user_steps_and_modal_scopes_take_precedence() {
        let tree = Node::row(vec![
            Node::pane(0),
            Node::pane(1).when(ViewportRule {
                min_width: 80,
                ..ViewportRule::default()
            }),
            Node::pane(2).focus(FocusPolicy::Direct),
        ]);
        let wide = tree.layout(Rect::new(0, 0, 100, 20)).unwrap();
        let small = tree.layout(Rect::new(0, 0, 40, 20)).unwrap();
        let mut focus = Focus::new(Some(1));
        assert_eq!(focus.reconcile(&small), Some(0));
        assert_eq!(focus.reconcile(&wide), Some(1));
        focus.reconcile(&small);
        focus.step(&small, true);
        assert_eq!(focus.reconcile(&wide), Some(0));
        assert!(focus.push_scope(vec![10, 11]));
        focus.request(1);
        assert_eq!(
            focus.active(),
            Some(10),
            "background shortcuts cannot escape a modal"
        );
        assert_eq!(focus.step(&wide, false), Some(11));
        assert!(focus.push_scope(vec![20]));
        assert_eq!(focus.step(&wide, true), Some(20));
        assert_eq!(focus.pop_scope(&wide), Some(11));
        assert_eq!(focus.pop_scope(&wide), Some(0));
        focus.request(2);
        assert_eq!(focus.reconcile(&wide), Some(2));
        assert_eq!(focus.step(&wide, true), Some(0));
        assert!(!focus.push_scope(vec![]));
    }

    #[test]
    fn duplicate_ids_depth_and_group_collapse_are_rejected() {
        assert_eq!(
            Node::row(vec![Node::pane(0), Node::pane(0)]).layout(Rect::new(0, 0, 10, 10)),
            Err(LayoutError::DuplicateId)
        );
        let mut deep = Node::pane(0);
        for _ in 0..65 {
            deep = Node::column(vec![deep]);
        }
        assert_eq!(
            deep.layout(Rect::new(0, 0, 10, 10)),
            Err(LayoutError::TooDeep)
        );
        assert_eq!(
            Node::column(vec![Node::pane(0)])
                .visibility(Visibility::Collapsed)
                .layout(Rect::new(0, 0, 10, 10)),
            Err(LayoutError::CollapsedGroup)
        );
    }

    #[test]
    fn focus_policy_applies_to_whole_subtrees_and_node_counts_are_bounded() {
        let tree = Node::column(vec![
            Node::row(vec![Node::pane(0), Node::pane(1)]).focus(FocusPolicy::Never),
            Node::row(vec![Node::pane(2), Node::pane(3).focus(FocusPolicy::Never)])
                .focus(FocusPolicy::Direct),
            Node::pane(4),
        ]);
        let layout = tree.layout(Rect::new(0, 0, 100, 30)).unwrap();
        assert_eq!(layout.focus_order().collect::<Vec<_>>(), vec![4]);
        assert!(!layout.can_focus(0));
        assert!(layout.can_focus(2));
        assert!(!layout.can_focus(3));
        let huge = Node::row((0..4096).map(Node::pane).collect());
        assert_eq!(
            huge.layout(Rect::new(0, 0, 100, 30)),
            Err(LayoutError::TooManyNodes)
        );
    }

    #[test]
    fn deeply_nested_valid_trees_measure_without_exponential_work() {
        let mut tree = Node::pane(0);
        for _ in 0..60 {
            tree = Node::column(vec![tree]);
        }
        let layout = tree.layout(Rect::new(0, 0, 80, 20)).unwrap();
        assert_eq!(layout.rect_of(0), Some(Rect::new(0, 0, 80, 20)));
    }

    #[test]
    fn resizing_is_origin_based_bounded_and_safe_at_integer_extremes() {
        let capture = ResizeCapture::new(400, 12, 2, 20, ResizeEdge::Leading).step(16);
        assert_eq!(capture.size_at(336), 16);
        assert_eq!(capture.size_at(336), 16);
        assert_eq!(capture.size_at(i32::MIN), 20);
        assert_eq!(capture.size_at(i32::MAX), 2);
        assert_eq!(
            ResizeCapture::new(0, 10, 0, 20, ResizeEdge::Trailing)
                .step(0)
                .size_at(3),
            13
        );
    }

    #[test]
    fn agents_can_inspect_bounds_and_visibility_without_a_renderer() {
        let layout = Node::column(vec![
            Node::pane("main"),
            Node::pane("hidden").visibility(Visibility::Hidden),
        ])
        .layout(Rect::new(3, 4, 80, 20))
        .unwrap();
        let json = serde_json::to_value(layout).unwrap();
        assert_eq!(json["panes"][0]["area"]["width"], 80);
        assert!(json["panes"][1]["area"].is_null());
        assert_eq!(json["panes"][1]["state"]["reason"], "user");
    }

    proptest! {
        #[test]
        fn every_frame_is_bounded_disjoint_deterministic_and_hit_testable(
            values in prop::collection::vec((0u16..80, 0u16..100, 0u16..100, 0u16..8, any::<bool>(), any::<bool>()), 0..24),
            width in 0u16..300, height in 0u16..100, gap in 0u16..10, wrap in any::<bool>(), vertical in any::<bool>(),
        ) {
            let children: Vec<_> = values.iter().enumerate().map(|(id, &(min, preferred, max, priority, hide, collapse))|
                Node::pane(id).width(Size {min, preferred, max, grow: 1}).height(Size::bounded(2, 6, 12).grow(1))
                    .priority(priority).overflow(if collapse {Overflow::CollapseThenHide} else {Overflow::Keep})
                    .compact(Size::fill(1), Size::fixed(1))
                    .visibility(if hide {Visibility::Hidden} else {Visibility::Expanded})).collect();
            let tree = if vertical {Node::column(children)} else {Node::row(children)}
                .gap(gap).wrap(if wrap {Wrap::AtMinimum} else {Wrap::Never});
            let area = Rect::new(7, 11, width, height);
            let layout = tree.layout(area).unwrap();
            prop_assert_eq!(&layout, &tree.layout(area).unwrap());
            prop_assert_eq!(layout.panes.len(), values.len());
            for (index, pane) in layout.panes.iter().enumerate() {
                if let Some(r) = pane.area {
                    prop_assert!(r.x >= area.x && r.y >= area.y && r.right() <= area.right() && r.bottom() <= area.bottom());
                    prop_assert_eq!(layout.hit(r.x, r.y), Some(pane.id));
                    prop_assert_eq!(layout.hit(r.right()-1, r.bottom()-1), Some(pane.id));
                    for other in &layout.panes[index+1..] {
                        prop_assert!(!other.area.is_some_and(|b| r.intersects(b)));
                    }
                }
                if layout.fits { prop_assert!(!pane.state.clipped); }
            }
        }
    }
}
