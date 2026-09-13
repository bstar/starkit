//! The docked-panel layout tree.
//!
//! Both applications draw a Winamp-style stack of panels that the user opens,
//! closes, reorders by configuration and resizes by dragging the border
//! between two of them. Until now each of them solved that with a hand-written
//! `regions()` function: a list of `Constraint`s built in draw order, and a
//! parallel structure holding the rects so that mouse handling could ask where
//! things had ended up. That function is the one place a layout bug can hide,
//! because it is the only code that knows both halves.
//!
//! This is the same discipline, generalised. The layout is a tree, [`layout`]
//! is the single geometry call per frame, and its answer is what both drawing
//! and mouse handling read. Nothing else computes a rect.
//!
//! [`layout`]: Dock::layout
//!
//! ## Sizes
//!
//! Along the axis its parent splits on, a node is [`Size::Fixed`] -- so many
//! cells, always -- or [`Size::Flex`] with a weight, sharing what the fixed
//! children left over in proportion to the weights. [`Size::Fill`] is
//! `Flex(1)`, spelled for the common case of one child that takes the rest.
//! Space nobody asked for -- every child fixed, in a larger area -- goes to the
//! last child, because the rects have to tile.
//!
//! `min` is the smallest a leaf is useful at, measured along the same axis. A
//! split's minimum is the sum of its children's along its own axis and the
//! largest of them across it.
//!
//! ## Seams
//!
//! The seam between two adjacent siblings is the one cell of border at the
//! *leading edge of the second one*, not a row of its own. A panel draws a
//! box; that box's top or left edge is already there, and giving the seam a
//! cell of its own would either move every panel by a cell or make the rects
//! stop tiling. `layout` reports where the seams are; `seam_at` turns a
//! pointer position into one, and `drag_seam` moves it.
//!
//! ## Too small
//!
//! When the minimums do not fit, children are dropped from the end until they
//! do. A dropped leaf gets no rect and is named in [`Layout::too_small`]; so
//! is one that had to be placed below its minimum because there was no sibling
//! left to take space from. What never happens is a gap or an overlap: the
//! placed rects always tile the area exactly, whatever the area is.
//!
//! Dropping from the end is arbitrary, and deliberately so: this tree has no
//! way to rank panels. An application that cares which panel survives a narrow
//! terminal hides one itself before calling `layout` -- which is also how it
//! can tell the difference afterwards between a panel the user closed and one
//! the width closed.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::hash::Hash;

use ratatui::layout::{Position, Rect};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// What an application's panel enum has to be for the tree to hold it.
///
/// `Ord` is for the hidden set, which is ordered so that a saved config is
/// stable rather than in whatever order a hash gave. The serde bounds are for
/// [`DockConfig`]; deriving `Serialize`, `Deserialize`, `Clone`, `Copy`,
/// `PartialEq`, `Eq`, `PartialOrd`, `Ord` and `Hash` on a plain enum satisfies
/// all of it.
pub trait PanelId: Copy + Eq + Ord + Hash + Serialize + DeserializeOwned + fmt::Debug {}

impl<T> PanelId for T where T: Copy + Eq + Ord + Hash + Serialize + DeserializeOwned + fmt::Debug {}

/// Which way a split divides its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// Children stacked top to bottom, dividing the height.
    Vertical,
    /// Children side by side, dividing the width.
    Horizontal,
}

/// How much of its parent a node takes along the parent's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// Exactly this many cells.
    Fixed(u16),
    /// A share of what the fixed children left, by weight.
    Flex(u16),
    /// The rest. Identical to `Flex(1)`, and worth its own name where a tree
    /// has one obvious main panel.
    Fill,
}

impl Size {
    fn weight(self) -> u32 {
        match self {
            Size::Fixed(_) => 0,
            Size::Flex(w) => u32::from(w).max(1),
            Size::Fill => 1,
        }
    }

    fn is_flexible(self) -> bool {
        !matches!(self, Size::Fixed(_))
    }
}

/// `"12"` is twelve cells, `"3fr"` is a weight of three, `"fill"` is the rest.
///
/// A size lives in a config file that a person edits, so it is one word rather
/// than a tagged table.
impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Size::Fixed(n) => write!(f, "{n}"),
            Size::Flex(w) => write!(f, "{w}fr"),
            Size::Fill => f.write_str("fill"),
        }
    }
}

impl std::str::FromStr for Size {
    type Err = SizeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("fill") {
            return Ok(Size::Fill);
        }
        let err = || SizeParseError(s.to_string());
        match s.strip_suffix("fr").or_else(|| s.strip_suffix("FR")) {
            Some(w) => w.trim().parse().map(Size::Flex).map_err(|_| err()),
            None => s.parse().map(Size::Fixed).map_err(|_| err()),
        }
    }
}

/// A `size` in a config file that is not a number, a weight or `fill`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeParseError(String);

impl fmt::Display for SizeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a size: expected cells (\"24\"), a weight (\"3fr\") or \"fill\"",
            self.0
        )
    }
}

impl std::error::Error for SizeParseError {}

impl Serialize for Size {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Size {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A node of the layout tree: a panel, or a split holding more nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node<P> {
    Leaf {
        id: P,
        size: Size,
        min: u16,
    },
    Split {
        axis: Axis,
        children: Vec<Node<P>>,
        size: Size,
    },
}

impl<P: Copy> Node<P> {
    pub fn leaf(id: P, size: Size, min: u16) -> Self {
        Node::Leaf { id, size, min }
    }

    /// A row of children, side by side.
    pub fn row(size: Size, children: Vec<Node<P>>) -> Self {
        Node::Split {
            axis: Axis::Horizontal,
            children,
            size,
        }
    }

    /// A column of children, stacked.
    pub fn column(size: Size, children: Vec<Node<P>>) -> Self {
        Node::Split {
            axis: Axis::Vertical,
            children,
            size,
        }
    }

    fn size(&self) -> Size {
        match self {
            Node::Leaf { size, .. } | Node::Split { size, .. } => *size,
        }
    }

    fn set_size(&mut self, s: Size) {
        match self {
            Node::Leaf { size, .. } | Node::Split { size, .. } => *size = s,
        }
    }

    fn leaves(&self, hidden: &BTreeSet<P>, out: &mut Vec<P>)
    where
        P: Ord,
    {
        match self {
            Node::Leaf { id, .. } => {
                if !hidden.contains(id) {
                    out.push(*id);
                }
            }
            Node::Split { children, .. } => {
                for c in children {
                    c.leaves(hidden, out);
                }
            }
        }
    }

    /// Is everything under this node hidden? Such a node is not solved for and
    /// takes no space, which is what makes closing the last panel of a column
    /// close the column.
    fn is_hidden(&self, hidden: &BTreeSet<P>) -> bool
    where
        P: Ord,
    {
        match self {
            Node::Leaf { id, .. } => hidden.contains(id),
            Node::Split { children, .. } => children.iter().all(|c| c.is_hidden(hidden)),
        }
    }

    /// The smallest this node is useful at, measured along `axis`.
    fn min_along(&self, axis: Axis, hidden: &BTreeSet<P>) -> u16
    where
        P: Ord,
    {
        match self {
            Node::Leaf { min, .. } => *min,
            Node::Split {
                axis: own,
                children,
                ..
            } => {
                let vis = children.iter().filter(|c| !c.is_hidden(hidden));
                if *own == axis {
                    vis.map(|c| c.min_along(axis, hidden)).sum()
                } else {
                    vis.map(|c| c.min_along(axis, hidden)).max().unwrap_or(0)
                }
            }
        }
    }
}

/// Where one panel ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed<P> {
    pub id: P,
    pub area: Rect,
}

/// The border between two adjacent siblings, and the handle for dragging it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seam {
    /// The one cell of border, which is the leading row or column of the
    /// second of the two panels.
    pub area: Rect,
    /// The axis of the split it divides: `Horizontal` seams are vertical lines
    /// and move left and right.
    pub axis: Axis,
    /// The split this seam is inside, as child indices from the root.
    path: Vec<usize>,
    /// Which of the split's *placed* children is the one after the seam.
    after: usize,
    a: u16,
    b: u16,
    a_min: u16,
    b_min: u16,
}

impl Seam {
    /// The extents of the two panels either side, as of the last layout.
    pub fn extents(&self) -> (u16, u16) {
        (self.a, self.b)
    }
}

/// The answer to one frame's geometry question.
///
/// `placed` tiles the area exactly. `too_small` names the panels the area
/// could not accommodate: they are either absent from `placed` entirely, or
/// present with less than their `min`. See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout<P> {
    pub placed: Vec<Placed<P>>,
    pub too_small: Vec<P>,
}

impl<P: Copy + Eq> Layout<P> {
    pub fn rect_of(&self, id: P) -> Option<Rect> {
        self.placed.iter().find(|p| p.id == id).map(|p| p.area)
    }
}

/// What one split resolved to last time, so a drag knows what it is moving.
#[derive(Debug, Clone)]
struct Solved {
    /// Indices into the split's `children`, in order, of the ones placed.
    kept: Vec<usize>,
    /// Their extents along the split's axis, aligned with `kept`.
    extents: Vec<u16>,
}

/// A tree of panels and the state that goes with it.
#[derive(Debug, Clone)]
pub struct Dock<P> {
    root: Node<P>,
    hidden: BTreeSet<P>,
    focus: Option<P>,
    seams: Vec<Seam>,
    rects: Vec<Placed<P>>,
    splits: HashMap<Vec<usize>, Solved>,
    area: Rect,
}

impl<P: PanelId> Dock<P> {
    pub fn new(root: Node<P>) -> Self {
        let mut d = Self {
            root,
            hidden: BTreeSet::new(),
            focus: None,
            seams: Vec::new(),
            rects: Vec::new(),
            splits: HashMap::new(),
            area: Rect::ZERO,
        };
        d.focus = d.visible_order().first().copied();
        d
    }

    pub fn root(&self) -> &Node<P> {
        &self.root
    }

    /// Solve the tree for `area`. The one geometry call per frame.
    pub fn layout(&mut self, area: Rect) -> Layout<P> {
        self.area = area;
        self.rects.clear();
        self.seams.clear();
        self.splits.clear();

        let mut too_small = Vec::new();
        if area.width == 0 || area.height == 0 {
            too_small = self.visible_order();
            return Layout {
                placed: Vec::new(),
                too_small,
            };
        }

        // The root's own `size` is not used: it has no sibling to share with.
        let hidden = std::mem::take(&mut self.hidden);
        let mut placed = Vec::new();
        let mut dropped = Vec::new();
        if !self.root.is_hidden(&hidden) {
            let root = std::mem::replace(
                &mut self.root,
                Node::Split {
                    axis: Axis::Vertical,
                    children: Vec::new(),
                    size: Size::Fill,
                },
            );
            let mut ctx = Solve {
                hidden: &hidden,
                placed: &mut placed,
                dropped: &mut dropped,
                seams: &mut self.seams,
                splits: &mut self.splits,
            };
            ctx.place(&root, area, &mut Vec::new());
            self.root = root;
        } else {
            dropped = {
                let mut v = Vec::new();
                self.root.leaves(&hidden, &mut v);
                v
            };
        }
        self.hidden = hidden;

        // A squeezed split reports every leaf under it, and a leaf under two
        // squeezed ancestors would otherwise be named twice.
        let mut seen = BTreeSet::new();
        too_small.extend(dropped.into_iter().filter(|p| seen.insert(*p)));
        self.rects = placed.clone();
        Layout { placed, too_small }
    }

    /// The rect of a panel as of the last [`Dock::layout`].
    pub fn rect_of(&self, id: P) -> Option<Rect> {
        self.rects.iter().find(|p| p.id == id).map(|p| p.area)
    }

    /// Which panel is under this cell.
    pub fn hit(&self, x: u16, y: u16) -> Option<P> {
        self.rects
            .iter()
            .find(|p| p.area.contains(Position { x, y }))
            .map(|p| p.id)
    }

    /// The seams, in the order [`Dock::seam_at`] and [`Dock::drag_seam`] index
    /// them.
    pub fn seams(&self) -> &[Seam] {
        &self.seams
    }

    /// Which seam is under this cell, if any.
    ///
    /// Seams are tested before panels: the cell is inside a panel as well, and
    /// a press on a panel border is a drag rather than a click on whatever the
    /// panel draws there.
    pub fn seam_at(&self, x: u16, y: u16) -> Option<usize> {
        self.seams
            .iter()
            .position(|s| s.area.contains(Position { x, y }))
    }

    /// Move a seam by `delta` cells, taking the space from one neighbour and
    /// giving it to the other.
    ///
    /// Neither neighbour goes below its minimum, and moving a seam back by the
    /// same amount puts the layout back exactly where it was. That is why this
    /// rewrites weights rather than nudging them: every flexible child of the
    /// split is given the weight of the extent it currently has, so the
    /// proportions that are not being dragged come out of the next solve
    /// unchanged, cell for cell.
    ///
    /// It acts on the geometry of the last [`Dock::layout`], which is the
    /// frame the pointer is in.
    pub fn drag_seam(&mut self, seam: usize, delta: i16) {
        let Some(s) = self.seams.get(seam).cloned() else {
            return;
        };
        let Some(solved) = self.splits.get(&s.path).cloned() else {
            return;
        };
        let pair = i32::from(s.a) + i32::from(s.b);
        let lo = i32::from(s.a_min);
        let hi = pair - i32::from(s.b_min);
        if lo > hi {
            // The two of them do not both fit as it is; moving the seam can
            // only make that worse.
            return;
        }
        let a = (i32::from(s.a) + i32::from(delta)).clamp(lo, hi) as u16;
        let b = (pair as u16) - a;
        if a == s.a {
            return;
        }

        let mut extents = solved.extents.clone();
        extents[s.after - 1] = a;
        extents[s.after] = b;

        let Some(node) = node_at_mut(&mut self.root, &s.path) else {
            return;
        };
        let Node::Split { children, .. } = node else {
            return;
        };
        for (k, &ci) in solved.kept.iter().enumerate() {
            let child = &mut children[ci];
            let e = extents[k].max(1);
            if child.size().is_flexible() {
                child.set_size(Size::Flex(e));
            } else if k == s.after || k + 1 == s.after {
                child.set_size(Size::Fixed(e));
            }
        }

        let area = self.area;
        self.layout(area);
    }

    /// Panels in the order focus moves through them: the visible leaves,
    /// depth first, which is the order they are drawn in.
    pub fn visible_order(&self) -> Vec<P> {
        let mut v = Vec::new();
        self.root.leaves(&self.hidden, &mut v);
        v
    }

    pub fn is_visible(&self, id: P) -> bool {
        !self.hidden.contains(&id)
    }

    pub fn hidden(&self) -> impl Iterator<Item = &P> {
        self.hidden.iter()
    }

    pub fn show(&mut self, id: P) {
        self.hidden.remove(&id);
    }

    /// Hide a panel. Its siblings' sizes are not touched, so showing it again
    /// restores the layout the user had rather than one the solver invented.
    pub fn hide(&mut self, id: P) {
        self.hidden.insert(id);
        if self.focus == Some(id) {
            self.focus = self.visible_order().first().copied();
        }
    }

    pub fn toggle(&mut self, id: P) {
        if self.is_visible(id) {
            self.hide(id);
        } else {
            self.show(id);
        }
    }

    pub fn focus(&self) -> Option<P> {
        self.focus.filter(|f| self.is_visible(*f))
    }

    /// Focus a panel, opening it if it was closed: `alt+3` should go to the
    /// third panel whether or not it is on screen.
    pub fn focus_set(&mut self, id: P) {
        self.show(id);
        self.focus = Some(id);
    }

    pub fn focus_next(&mut self) {
        self.step_focus(1);
    }

    pub fn focus_prev(&mut self) {
        self.step_focus(-1);
    }

    fn step_focus(&mut self, by: isize) {
        let order = self.visible_order();
        if order.is_empty() {
            self.focus = None;
            return;
        }
        let at = self.focus.and_then(|f| order.iter().position(|p| *p == f));
        let next = match at {
            Some(i) => (i as isize + by).rem_euclid(order.len() as isize) as usize,
            None if by >= 0 => 0,
            None => order.len() - 1,
        };
        self.focus = Some(order[next]);
    }

    /// The tree and its state, in the shape that goes in a config file.
    pub fn to_config(&self) -> DockConfig<P> {
        DockConfig {
            hidden: self.hidden.iter().copied().collect(),
            focus: self.focus,
            root: NodeConfig::from_node(&self.root),
        }
    }

    /// Rebuild from a config. A `hidden` entry naming a panel that is not in
    /// the tree is kept rather than dropped, so that a config written by a
    /// newer version survives a downgrade.
    pub fn from_config(cfg: DockConfig<P>) -> Result<Self, ConfigError> {
        let root = cfg.root.into_node()?;
        let mut d = Self {
            root,
            hidden: cfg.hidden.into_iter().collect(),
            focus: cfg.focus,
            seams: Vec::new(),
            rects: Vec::new(),
            splits: HashMap::new(),
            area: Rect::ZERO,
        };
        if d.focus.is_none() {
            d.focus = d.visible_order().first().copied();
        }
        Ok(d)
    }
}

/// The recursive walk, with the pieces it writes into.
struct Solve<'a, P> {
    hidden: &'a BTreeSet<P>,
    placed: &'a mut Vec<Placed<P>>,
    dropped: &'a mut Vec<P>,
    seams: &'a mut Vec<Seam>,
    splits: &'a mut HashMap<Vec<usize>, Solved>,
}

impl<P: PanelId> Solve<'_, P> {
    fn place(&mut self, node: &Node<P>, area: Rect, path: &mut Vec<usize>) {
        match node {
            Node::Leaf { id, .. } => self.placed.push(Placed { id: *id, area }),
            Node::Split { axis, children, .. } => {
                let vis: Vec<usize> = (0..children.len())
                    .filter(|&i| !children[i].is_hidden(self.hidden))
                    .collect();
                if vis.is_empty() {
                    return;
                }

                let along = match axis {
                    Axis::Vertical => area.height,
                    Axis::Horizontal => area.width,
                };
                let sizes: Vec<Size> = vis.iter().map(|&i| children[i].size()).collect();
                let mins: Vec<u16> = vis
                    .iter()
                    .map(|&i| children[i].min_along(*axis, self.hidden).max(1))
                    .collect();

                // Drop from the end until the minimums fit. One child always
                // survives: a cramped panel beats an empty screen.
                let mut keep = vis.len();
                while keep > 1
                    && mins[..keep].iter().map(|m| u32::from(*m)).sum::<u32>() > u32::from(along)
                {
                    keep -= 1;
                }
                for &i in &vis[keep..] {
                    children[i].leaves(self.hidden, self.dropped);
                }

                let extents = solve(&sizes[..keep], &mins[..keep], along);
                self.splits.insert(
                    path.clone(),
                    Solved {
                        kept: vis[..keep].to_vec(),
                        extents: extents.clone(),
                    },
                );

                let mut at = match axis {
                    Axis::Vertical => area.y,
                    Axis::Horizontal => area.x,
                };
                for (k, &ci) in vis[..keep].iter().enumerate() {
                    let e = extents[k];
                    if e < mins[k] {
                        // Placed, but smaller than it says it needs: there was
                        // no sibling left with room to give.
                        children[ci].leaves(self.hidden, self.dropped);
                    }
                    let rect = match axis {
                        Axis::Vertical => Rect {
                            x: area.x,
                            y: at,
                            width: area.width,
                            height: e,
                        },
                        Axis::Horizontal => Rect {
                            x: at,
                            y: area.y,
                            width: e,
                            height: area.height,
                        },
                    };
                    if k > 0 && e > 0 {
                        // The seam lives inside the second panel's leading
                        // edge, so the rects still tile.
                        let sa = match axis {
                            Axis::Vertical => Rect { height: 1, ..rect },
                            Axis::Horizontal => Rect { width: 1, ..rect },
                        };
                        self.seams.push(Seam {
                            area: sa,
                            axis: *axis,
                            path: path.clone(),
                            after: k,
                            a: extents[k - 1],
                            b: e,
                            a_min: mins[k - 1],
                            b_min: mins[k],
                        });
                    }
                    at += e;
                    path.push(ci);
                    self.place(&children[ci], rect, path);
                    path.pop();
                }
            }
        }
    }
}

/// Divide `along` cells between children that are fixed, flexible, or both.
///
/// The result always sums to `along`, which is the tiling invariant; when the
/// minimums cannot all be met somebody ends up below theirs, and the caller
/// reports that rather than leaving a hole.
fn solve(sizes: &[Size], mins: &[u16], along: u16) -> Vec<u16> {
    let n = sizes.len();
    let along = u32::from(along);
    let mut out = vec![0u32; n];
    if n == 0 {
        return Vec::new();
    }

    let mut fixed_total = 0u32;
    for i in 0..n {
        if let Size::Fixed(v) = sizes[i] {
            out[i] = u32::from(v).max(u32::from(mins[i]));
            fixed_total += out[i];
        }
    }
    // Fixed children that do not fit shrink from the end: towards their
    // minimums first, and past them only if even those do not fit.
    for floor_at_min in [true, false] {
        if fixed_total <= along {
            break;
        }
        let mut over = fixed_total - along;
        for i in (0..n).rev() {
            if over == 0 {
                break;
            }
            if matches!(sizes[i], Size::Fixed(_)) {
                let floor = if floor_at_min { u32::from(mins[i]) } else { 0 };
                let give = out[i].saturating_sub(floor).min(over);
                out[i] -= give;
                over -= give;
                fixed_total -= give;
            }
        }
    }

    let space = along.saturating_sub(fixed_total);
    let flex: Vec<usize> = (0..n).filter(|&i| sizes[i].is_flexible()).collect();
    if flex.is_empty() {
        // Nobody asked for the rest, and the rects still have to tile.
        out[n - 1] += space;
        return out
            .into_iter()
            .map(|v| v.min(u32::from(u16::MAX)) as u16)
            .collect();
    }

    let total_w: u32 = flex.iter().map(|&i| sizes[i].weight()).sum();
    let mut used = 0u32;
    for (k, &i) in flex.iter().enumerate() {
        let share = if k + 1 == flex.len() {
            space - used
        } else {
            space * sizes[i].weight() / total_w
        };
        out[i] = share;
        used += share;
    }

    // Raise the flexible children that came out below their minimum, taking
    // the cells from those with room above theirs. Only as far as that room
    // goes: the total is fixed, and the shortfall is the caller's to report.
    let deficit: u32 = flex
        .iter()
        .map(|&i| u32::from(mins[i]).saturating_sub(out[i]))
        .sum();
    if deficit > 0 {
        let surplus: u32 = flex
            .iter()
            .map(|&i| out[i].saturating_sub(u32::from(mins[i])))
            .sum();
        let mut pool = deficit.min(surplus);
        let taken = pool;
        for &i in &flex {
            if pool == 0 {
                break;
            }
            let room = out[i].saturating_sub(u32::from(mins[i]));
            let take = room.min(pool);
            out[i] -= take;
            pool -= take;
        }
        let mut give = taken;
        for &i in &flex {
            if give == 0 {
                break;
            }
            let want = u32::from(mins[i]).saturating_sub(out[i]);
            let g = want.min(give);
            out[i] += g;
            give -= g;
        }
    }

    out.into_iter()
        .map(|v| v.min(u32::from(u16::MAX)) as u16)
        .collect()
}

fn node_at_mut<'a, P>(root: &'a mut Node<P>, path: &[usize]) -> Option<&'a mut Node<P>> {
    let mut node = root;
    for &i in path {
        match node {
            Node::Split { children, .. } => node = children.get_mut(i)?,
            Node::Leaf { .. } => return None,
        }
    }
    Some(node)
}

// -- config ----------------------------------------------------------------

/// A dock as it appears in a config file.
///
/// ```toml
/// hidden = ["members"]
/// focus = "chat"
///
/// [root]
/// axis = "vertical"
/// size = "fill"
///
/// [[root.children]]
/// panel = "chat"
/// size = "1fr"
/// min = 4
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockConfig<P> {
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<P>,
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub focus: Option<P>,
    pub root: NodeConfig<P>,
}

/// One node of the tree in a config file: either `panel`, or `axis` plus
/// `children`.
///
/// The scalar fields come first because that is the order they are written in,
/// and a TOML table may not have a value after a subtable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConfig<P> {
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub panel: Option<P>,
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub axis: Option<Axis>,
    pub size: Size,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min: u16,
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeConfig<P>>,
}

fn none<T>() -> Option<T> {
    None
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

/// A config that does not describe a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A node names both a panel and an axis, or neither.
    NotOneOrTheOther,
    /// A split with no children, which would take space and draw nothing.
    EmptySplit,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotOneOrTheOther => {
                f.write_str("a layout node is either a `panel` or an `axis` with `children`")
            }
            ConfigError::EmptySplit => f.write_str("a layout split has no children"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl<P: Copy> NodeConfig<P> {
    fn from_node(node: &Node<P>) -> Self {
        match node {
            Node::Leaf { id, size, min } => NodeConfig {
                panel: Some(*id),
                axis: None,
                size: *size,
                min: *min,
                children: Vec::new(),
            },
            Node::Split {
                axis,
                children,
                size,
            } => NodeConfig {
                panel: None,
                axis: Some(*axis),
                size: *size,
                min: 0,
                children: children.iter().map(NodeConfig::from_node).collect(),
            },
        }
    }

    fn into_node(self) -> Result<Node<P>, ConfigError> {
        match (self.panel, self.axis) {
            (Some(id), None) => Ok(Node::Leaf {
                id,
                size: self.size,
                min: self.min,
            }),
            (None, Some(axis)) => {
                if self.children.is_empty() {
                    return Err(ConfigError::EmptySplit);
                }
                Ok(Node::Split {
                    axis,
                    children: self
                        .children
                        .into_iter()
                        .map(NodeConfig::into_node)
                        .collect::<Result<_, _>>()?,
                    size: self.size,
                })
            }
            _ => Err(ConfigError::NotOneOrTheOther),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Panel {
        Guilds,
        Channels,
        Dms,
        Chat,
        Composer,
        Members,
        Status,
    }
    use Panel::*;

    /// The tree the chat application actually uses, which is the one worth
    /// testing the interesting cases against.
    fn chat_dock() -> Dock<Panel> {
        Dock::new(Node::column(
            Size::Fill,
            vec![
                Node::row(
                    Size::Fill,
                    vec![
                        Node::leaf(Guilds, Size::Fixed(8), 8),
                        Node::column(
                            Size::Fixed(26),
                            vec![
                                Node::leaf(Channels, Size::Flex(3), 3),
                                Node::leaf(Dms, Size::Flex(2), 3),
                            ],
                        ),
                        Node::column(
                            Size::Flex(1),
                            vec![
                                Node::leaf(Chat, Size::Flex(1), 4),
                                Node::leaf(Composer, Size::Fixed(3), 3),
                            ],
                        ),
                        Node::leaf(Members, Size::Fixed(24), 18),
                    ],
                ),
                Node::leaf(Status, Size::Fixed(1), 1),
            ],
        ))
    }

    const FULL: Rect = Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };

    fn tiles(area: Rect, placed: &[Placed<Panel>]) {
        let mut grid = vec![0u8; usize::from(area.width) * usize::from(area.height)];
        for p in placed {
            assert!(
                p.area.x >= area.x
                    && p.area.y >= area.y
                    && p.area.x + p.area.width <= area.x + area.width
                    && p.area.y + p.area.height <= area.y + area.height,
                "{:?} is outside {area:?}",
                p.area
            );
            for y in p.area.y..p.area.y + p.area.height {
                for x in p.area.x..p.area.x + p.area.width {
                    let i =
                        usize::from(y - area.y) * usize::from(area.width) + usize::from(x - area.x);
                    grid[i] += 1;
                }
            }
        }
        assert!(
            grid.iter().all(|c| *c == 1),
            "the placed rects do not tile the area exactly"
        );
    }

    #[test]
    fn the_chat_layout_comes_out_the_way_it_is_written() {
        let mut d = chat_dock();
        let l = d.layout(FULL);
        assert!(l.too_small.is_empty());
        tiles(FULL, &l.placed);

        assert_eq!(l.rect_of(Guilds).unwrap(), Rect::new(0, 0, 8, 39));
        // 39 rows shared three to two: 23 and the 16 that are left.
        assert_eq!(l.rect_of(Channels).unwrap(), Rect::new(8, 0, 26, 23));
        assert_eq!(l.rect_of(Dms).unwrap(), Rect::new(8, 23, 26, 16));
        assert_eq!(l.rect_of(Chat).unwrap(), Rect::new(34, 0, 62, 36));
        assert_eq!(l.rect_of(Composer).unwrap(), Rect::new(34, 36, 62, 3));
        assert_eq!(l.rect_of(Members).unwrap(), Rect::new(96, 0, 24, 39));
        assert_eq!(l.rect_of(Status).unwrap(), Rect::new(0, 39, 120, 1));
    }

    #[test]
    fn a_seam_is_the_first_cell_of_the_panel_after_it() {
        let mut d = chat_dock();
        d.layout(FULL);
        let chat = d.rect_of(Chat).unwrap();
        let seam = d
            .seams()
            .iter()
            .find(|s| s.area.x == chat.x && s.axis == Axis::Horizontal)
            .expect("a seam at the left edge of the chat column");
        assert_eq!(seam.area.width, 1);
        assert_eq!(seam.area.height, 39);
        assert_eq!(
            d.seam_at(chat.x, 5),
            Some(d.seams().iter().position(|s| s == seam).unwrap())
        );
    }

    #[test]
    fn hit_testing_finds_the_panel_the_cell_is_in() {
        let mut d = chat_dock();
        d.layout(FULL);
        assert_eq!(d.hit(0, 0), Some(Guilds));
        assert_eq!(d.hit(9, 1), Some(Channels));
        assert_eq!(d.hit(9, 30), Some(Dms));
        assert_eq!(d.hit(50, 37), Some(Composer));
        assert_eq!(d.hit(119, 39), Some(Status));
        assert_eq!(d.hit(200, 200), None);
    }

    #[test]
    fn dragging_a_seam_moves_exactly_that_many_cells() {
        let mut d = chat_dock();
        d.layout(FULL);
        let before = d.rect_of(Channels).unwrap();
        let seam = d
            .seams()
            .iter()
            .position(|s| s.area.y == before.y + before.height && s.axis == Axis::Vertical)
            .expect("the seam between the channel list and the direct messages");
        d.drag_seam(seam, 4);
        assert_eq!(d.rect_of(Channels).unwrap().height, before.height + 4);
        assert_eq!(d.rect_of(Dms).unwrap().height, 16 - 4);
    }

    #[test]
    fn dragging_a_seam_back_puts_the_layout_back() {
        let mut d = chat_dock();
        let before = d.layout(FULL);
        let seam = d
            .seams()
            .iter()
            .position(|s| s.axis == Axis::Vertical)
            .unwrap();
        d.drag_seam(seam, 5);
        assert_ne!(d.layout(FULL), before);
        d.drag_seam(seam, -5);
        assert_eq!(d.layout(FULL), before);
    }

    #[test]
    fn a_seam_never_pushes_a_neighbour_below_its_minimum() {
        let mut d = chat_dock();
        d.layout(FULL);
        let seam = d
            .seams()
            .iter()
            .position(|s| s.axis == Axis::Vertical)
            .unwrap();
        for delta in [100i16, 100, -100, -100, 3, -50] {
            d.drag_seam(seam, delta);
            let l = d.layout(FULL);
            assert!(l.rect_of(Channels).unwrap().height >= 3);
            assert!(l.rect_of(Dms).unwrap().height >= 3);
            tiles(FULL, &l.placed);
        }
    }

    #[test]
    fn dragging_a_fixed_panel_makes_it_a_different_fixed_panel() {
        // The member list is Fixed(24); after a drag it is still fixed, at the
        // width the pointer left it at, so a resize of the terminal does not
        // undo the drag.
        let mut d = chat_dock();
        d.layout(FULL);
        let members = d.rect_of(Members).unwrap();
        let seam = d
            .seams()
            .iter()
            .position(|s| s.area.x == members.x && s.axis == Axis::Horizontal)
            .unwrap();
        d.drag_seam(seam, -6);
        assert_eq!(d.rect_of(Members).unwrap().width, 30);
        let wider = d.layout(Rect::new(0, 0, 160, 40));
        assert_eq!(wider.rect_of(Members).unwrap().width, 30);
    }

    #[test]
    fn hiding_a_panel_gives_its_space_to_its_siblings_and_showing_it_gives_it_back() {
        let mut d = chat_dock();
        let before = d.layout(FULL);
        d.hide(Members);
        let without = d.layout(FULL);
        assert!(without.rect_of(Members).is_none());
        assert!(without.rect_of(Chat).unwrap().width > before.rect_of(Chat).unwrap().width);
        tiles(FULL, &without.placed);
        d.show(Members);
        assert_eq!(d.layout(FULL), before, "showing restores the weights");
    }

    #[test]
    fn hiding_every_panel_of_a_column_hides_the_column() {
        let mut d = chat_dock();
        d.hide(Channels);
        d.hide(Dms);
        let l = d.layout(FULL);
        // The left column is gone entirely: the guild rail is next to chat.
        assert_eq!(l.rect_of(Chat).unwrap().x, 8);
        tiles(FULL, &l.placed);
    }

    #[test]
    fn hiding_a_panel_after_a_drag_keeps_the_dragged_size() {
        let mut d = chat_dock();
        d.layout(FULL);
        let seam = d
            .seams()
            .iter()
            .position(|s| s.axis == Axis::Vertical)
            .unwrap();
        d.drag_seam(seam, 4);
        let dragged = d.layout(FULL);
        d.hide(Members);
        d.layout(FULL);
        d.show(Members);
        assert_eq!(d.layout(FULL), dragged);
    }

    #[test]
    fn focus_walks_the_visible_panels_in_drawing_order_and_wraps() {
        let mut d = chat_dock();
        assert_eq!(d.focus(), Some(Guilds));
        let order = d.visible_order();
        assert_eq!(
            order,
            vec![Guilds, Channels, Dms, Chat, Composer, Members, Status]
        );
        for expected in order.iter().skip(1).chain(order.first()) {
            d.focus_next();
            assert_eq!(d.focus(), Some(*expected));
        }
        d.focus_prev();
        assert_eq!(d.focus(), Some(Status));
    }

    #[test]
    fn focus_skips_hidden_panels_and_leaves_one_that_is_closed() {
        let mut d = chat_dock();
        d.hide(Channels);
        d.focus_set(Guilds);
        d.focus_next();
        assert_eq!(d.focus(), Some(Dms));

        d.focus_set(Dms);
        d.hide(Dms);
        assert_eq!(
            d.focus(),
            Some(Guilds),
            "focus does not sit on a closed panel"
        );
    }

    #[test]
    fn focusing_a_closed_panel_opens_it() {
        let mut d = chat_dock();
        d.hide(Members);
        d.focus_set(Members);
        assert!(d.is_visible(Members));
        assert_eq!(d.focus(), Some(Members));
    }

    #[test]
    fn panels_that_do_not_fit_are_named_and_the_rest_still_tile() {
        let mut d = chat_dock();
        let small = Rect::new(0, 0, 30, 8);
        let l = d.layout(small);
        assert!(!l.too_small.is_empty(), "30x8 cannot hold this layout");
        tiles(small, &l.placed);
        for p in &l.placed {
            if !l.too_small.contains(&p.id) {
                assert!(p.area.width > 0 && p.area.height > 0);
            }
        }
    }

    #[test]
    fn a_panel_placed_below_its_minimum_is_reported_rather_than_dropped() {
        let mut d = Dock::new(Node::column(
            Size::Fill,
            vec![Node::leaf(Chat, Size::Fill, 10)],
        ));
        let area = Rect::new(0, 0, 20, 4);
        let l = d.layout(area);
        assert_eq!(l.rect_of(Chat).unwrap(), area, "it is still drawn");
        assert_eq!(l.too_small, vec![Chat]);
    }

    #[test]
    fn a_zero_sized_area_places_nothing_and_reports_everything() {
        let mut d = chat_dock();
        let l = d.layout(Rect::ZERO);
        assert!(l.placed.is_empty());
        assert_eq!(l.too_small.len(), 7);
    }

    #[test]
    fn space_nobody_asked_for_goes_to_the_last_child() {
        let mut d = Dock::new(Node::column(
            Size::Fill,
            vec![
                Node::leaf(Chat, Size::Fixed(4), 1),
                Node::leaf(Status, Size::Fixed(1), 1),
            ],
        ));
        let area = Rect::new(0, 0, 10, 20);
        let l = d.layout(area);
        assert_eq!(l.rect_of(Chat).unwrap().height, 4);
        assert_eq!(l.rect_of(Status).unwrap().height, 16);
        tiles(area, &l.placed);
    }

    #[test]
    fn the_config_round_trips_through_toml() {
        let mut d = chat_dock();
        d.hide(Members);
        d.focus_set(Chat);
        d.layout(FULL);
        let seam = d
            .seams()
            .iter()
            .position(|s| s.axis == Axis::Vertical)
            .unwrap();
        d.drag_seam(seam, 3);
        let before = d.layout(FULL);

        let text = toml::to_string(&d.to_config()).expect("serialises");
        let cfg: DockConfig<Panel> = toml::from_str(&text).expect("parses");
        let mut back = Dock::from_config(cfg).expect("a tree");

        assert_eq!(back.layout(FULL), before);
        assert_eq!(back.focus(), Some(Chat));
        assert!(!back.is_visible(Members));
        assert_eq!(back.to_config(), d.to_config());
    }

    #[test]
    fn a_size_is_one_word_in_the_file() {
        let text = toml::to_string(&chat_dock().to_config()).unwrap();
        assert!(text.contains("size = \"8\""), "{text}");
        assert!(text.contains("size = \"3fr\""), "{text}");
        assert!(text.contains("size = \"fill\""), "{text}");
        assert!(text.contains("panel = \"guilds\""), "{text}");
    }

    #[test]
    fn sizes_parse_the_way_they_print() {
        for s in [Size::Fixed(0), Size::Fixed(26), Size::Flex(1), Size::Fill] {
            assert_eq!(s.to_string().parse::<Size>().unwrap(), s);
        }
        assert!("".parse::<Size>().is_err());
        assert!("wide".parse::<Size>().is_err());
        assert!("-3".parse::<Size>().is_err());
    }

    #[test]
    fn a_node_that_is_neither_a_panel_nor_a_split_is_an_error() {
        let cfg: DockConfig<Panel> = toml::from_str("[root]\nsize = \"fill\"\n").unwrap();
        assert_eq!(
            Dock::from_config(cfg).unwrap_err(),
            ConfigError::NotOneOrTheOther
        );
        let cfg: DockConfig<Panel> =
            toml::from_str("[root]\naxis = \"vertical\"\nsize = \"fill\"\n").unwrap();
        assert_eq!(Dock::from_config(cfg).unwrap_err(), ConfigError::EmptySplit);
    }

    // -- generated trees ---------------------------------------------------

    /// A tree of at most eight leaves and four levels, with the panel ids
    /// assigned afterwards so that each appears exactly once.
    fn any_node(depth: u32) -> impl Strategy<Value = Node<u8>> {
        let leaf = (any_size(), 0u16..6).prop_map(|(size, min)| Node::Leaf { id: 0, size, min });
        leaf.prop_recursive(depth, 8, 4, |inner| {
            (
                prop_oneof![Just(Axis::Vertical), Just(Axis::Horizontal)],
                proptest::collection::vec(inner, 1..4),
                any_size(),
            )
                .prop_map(|(axis, children, size)| Node::Split {
                    axis,
                    children,
                    size,
                })
        })
    }

    fn any_size() -> impl Strategy<Value = Size> {
        prop_oneof![
            (0u16..30).prop_map(Size::Fixed),
            (1u16..5).prop_map(Size::Flex),
            Just(Size::Fill),
        ]
    }

    fn number_leaves(node: &mut Node<u8>, next: &mut u8) {
        match node {
            Node::Leaf { id, .. } => {
                *id = *next;
                *next += 1;
            }
            Node::Split { children, .. } => {
                for c in children {
                    number_leaves(c, next);
                }
            }
        }
    }

    fn covers(area: Rect, placed: &[Placed<u8>]) -> bool {
        let mut grid = vec![0u8; usize::from(area.width) * usize::from(area.height)];
        for p in placed {
            if p.area.x + p.area.width > area.x + area.width
                || p.area.y + p.area.height > area.y + area.height
            {
                return false;
            }
            for y in p.area.y..p.area.y + p.area.height {
                for x in p.area.x..p.area.x + p.area.width {
                    let i =
                        usize::from(y - area.y) * usize::from(area.width) + usize::from(x - area.x);
                    grid[i] += 1;
                }
            }
        }
        grid.iter().all(|c| *c == 1)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn any_tree_tiles_any_area(
            mut root in any_node(4),
            w in 1u16..90,
            h in 1u16..40,
            hide in proptest::collection::vec(0u8..8, 0..3),
        ) {
            number_leaves(&mut root, &mut 0);
            let mut d = Dock::new(root);
            let all = d.visible_order();
            for p in hide {
                if all.contains(&p) { d.hide(p); }
            }
            if d.visible_order().is_empty() {
                return Ok(());
            }
            let area = Rect::new(3, 2, w, h);
            let l = d.layout(area);
            prop_assert!(covers(area, &l.placed), "{:?} does not tile {area:?}", l.placed);

            // Every visible panel is either placed or reported, never neither.
            for p in d.visible_order() {
                prop_assert!(
                    l.rect_of(p).is_some() || l.too_small.contains(&p),
                    "panel {p} vanished"
                );
            }
            prop_assert!(l.placed.len() <= d.visible_order().len());
        }

        #[test]
        fn every_panel_is_placed_once_and_named_at_most_once(
            mut root in any_node(4),
            w in 1u16..200,
            h in 1u16..120,
        ) {
            number_leaves(&mut root, &mut 0);
            let mut d = Dock::new(root);
            let visible = d.visible_order();
            let l = d.layout(Rect::new(0, 0, w, h));

            let mut seen = std::collections::BTreeSet::new();
            for p in &l.placed {
                prop_assert!(seen.insert(p.id), "panel {} placed twice", p.id);
                prop_assert!(visible.contains(&p.id));
            }
            let mut named = std::collections::BTreeSet::new();
            for p in &l.too_small {
                prop_assert!(named.insert(*p), "panel {p} reported twice");
                prop_assert!(visible.contains(p));
            }
            // A panel with a rect smaller than one cell is always reported.
            for p in &l.placed {
                if p.area.width == 0 || p.area.height == 0 {
                    prop_assert!(l.too_small.contains(&p.id));
                }
            }
        }

        #[test]
        fn dragging_any_seam_keeps_the_tiling_and_the_minimums(
            mut root in any_node(4),
            w in 20u16..90,
            h in 12u16..40,
            which in 0usize..8,
            delta in -30i16..30,
        ) {
            number_leaves(&mut root, &mut 0);
            let mut d = Dock::new(root);
            let area = Rect::new(0, 0, w, h);
            let before = d.layout(area);
            if d.seams().is_empty() {
                return Ok(());
            }
            let seam = which % d.seams().len();
            let (a, b) = d.seams()[seam].extents();
            d.drag_seam(seam, delta);
            let after = d.layout(area);
            prop_assert!(covers(area, &after.placed));

            // The pair still adds up to what it did: a drag moves the border,
            // it does not take space from anybody else, and the same panels
            // are on screen afterwards. Only where everything fit to begin
            // with -- in an area too small for the minimums the solver is
            // already compromising, and a cell either way there decides
            // whether a panel is dropped at all.
            if before.too_small.is_empty() && after.too_small.is_empty() {
                prop_assert_eq!(before.placed.len(), after.placed.len());
                if let Some(s) = d.seams().get(seam) {
                    let (a2, b2) = s.extents();
                    prop_assert_eq!(a2 + b2, a + b);
                    prop_assert!(a2 >= s.a_min && b2 >= s.b_min, "a drag broke a minimum");
                }
            }
        }

        #[test]
        fn a_drag_and_its_opposite_cancel(
            mut root in any_node(3),
            w in 20u16..90,
            h in 12u16..40,
            which in 0usize..8,
            delta in 1i16..12,
        ) {
            number_leaves(&mut root, &mut 0);
            let mut d = Dock::new(root);
            let area = Rect::new(0, 0, w, h);
            let before = d.layout(area);
            if d.seams().is_empty() {
                return Ok(());
            }
            if !before.too_small.is_empty() {
                return Ok(());
            }
            let seam = which % d.seams().len();
            let (a0, _) = d.seams()[seam].extents();
            d.drag_seam(seam, delta);
            let moved = d.layout(area);
            // How far it actually went, which is less than asked for when the
            // drag ran into a minimum. Undoing a clamped drag means undoing
            // what happened, not what was asked for.
            let Some(applied) = d.seams().get(seam).map(|s| s.extents().0 as i32 - a0 as i32) else {
                return Ok(());
            };
            if moved.too_small.is_empty() && applied != 0 {
                d.drag_seam(seam, -applied as i16);
                prop_assert_eq!(d.layout(area), before);
            }
        }

        #[test]
        fn every_tree_round_trips_through_toml(mut root in any_node(4)) {
            number_leaves(&mut root, &mut 0);
            let mut d = Dock::new(root);
            let area = Rect::new(0, 0, 80, 24);
            let before = d.layout(area);
            let text = toml::to_string(&d.to_config()).expect("serialises");
            let cfg: DockConfig<u8> = toml::from_str(&text).expect("parses");
            let mut back = Dock::from_config(cfg).expect("a tree");
            prop_assert_eq!(back.layout(area), before);
        }

        #[test]
        fn hiding_and_showing_leaves_the_layout_where_it_was(
            mut root in any_node(4),
            which in 0usize..8,
        ) {
            number_leaves(&mut root, &mut 0);
            let mut d = Dock::new(root);
            let area = Rect::new(0, 0, 80, 24);
            let before = d.layout(area);
            let order = d.visible_order();
            let p = order[which % order.len()];
            d.hide(p);
            d.layout(area);
            d.show(p);
            prop_assert_eq!(d.layout(area), before);
        }
    }
}
