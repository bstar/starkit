//! One scrollbar for every list: a thumb on the panel's right border.
//!
//! Three lists in two applications used to draw their own version of this --
//! starcord's chat measured position through [`crate::vlist::VirtualList`]
//! because its items are not all the same height, staramp's playlist counted
//! rows because its items are. Both drew the same mark for the same reason, so
//! the mark lives here once, split into the part that is pure arithmetic
//! ([`thumb`]) and the two ways a caller gets to it ([`rows`] for a flat list,
//! [`virtual_list`] for one anchored through `VirtualList`).
//!
//! ## Why a full block, and why capped
//!
//! The thumb is drawn *on* the border, replacing a cell of it, rather than in
//! a column beside it -- that is what makes it a mark on the frame rather than
//! a fourth thing to fit into the panel's width. A double border's second
//! stroke sits on the right of the cell, which is where the right half of
//! [`GLYPH`] would fall and not where a half block's left-aligned stroke
//! falls, so anything but a full block reads as a notch bitten out of the
//! frame rather than a bead running down it.
//!
//! A thumb sized by the honest ratio of viewport to content is correct and
//! useless at the sizes these panels actually run at: a list a few rows
//! longer than fits covers most of the border with itself, which reads as a
//! second border rather than as a position indicator -- staramp's old test
//! for this called it exactly that. The other failure is a fixed one-row
//! bead, which is legible but throws away the one thing a size-proportional
//! thumb has over a scroll percentage: a sense of how much more there is.
//! [`THUMB_MAX_DIV`] is the compromise -- a quarter of the track still has a
//! silhouette that shrinks as the list grows, and still stops short of eating
//! the frame.
//!
//! ## Accent, focused or not
//!
//! The border's colour carries focus -- a docked panel that is not focused
//! draws a dim border, a focused one an accented one. The thumb does not
//! follow that: it is drawn in `accent` regardless, because it answers a
//! different question. The border says which panel is listening to the
//! keyboard; the thumb says where in its own content that panel is looking,
//! and a reader scrolled halfway down an unfocused panel still wants to see
//! that, exactly as they would a focused one.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::theme::Theme;

/// A full block, drawn on the border in place of whichever glyph is there.
///
/// See the module doc for why this and not a half block: a half block covers
/// the wrong stroke of a double border and reads as a notch rather than a
/// bead.
pub const GLYPH: &str = "\u{2588}";

/// The thumb is never longer than track / `THUMB_MAX_DIV`, and never shorter
/// than one row. See the module doc for why a cap and why a quarter.
pub const THUMB_MAX_DIV: u16 = 4;

/// A thumb's place on its track, in rows relative to the track's top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thumb {
    pub start: u16,
    pub len: u16,
}

/// The part of [`thumb`]'s arithmetic that does not depend on `above`: how
/// long the thumb is, how much track it has to travel over, and how much
/// content that travel stands for. [`grab`] and [`drag`] go through this too,
/// so the row the mouse lands on and the row [`render`] draws the thumb on
/// are never computed two different ways.
struct Geometry {
    len: u16,
    room: u16,
    /// Rows of content that are not the viewport -- `total - track` -- and so
    /// the range `above` moves over. Always positive: `None` below covers the
    /// only case where it would not be.
    scrolled: u64,
}

/// `None` when there is nothing to scroll -- the content fits in the track,
/// or there is no track to draw on.
fn geometry(track: u16, total: u32) -> Option<Geometry> {
    geometry_visible(track, total, u32::from(track), 1)
}
fn geometry_visible(track: u16, total: u32, visible: u32, minimum: u16) -> Option<Geometry> {
    if track == 0 || visible == 0 || total <= visible {
        return None;
    }
    let track64 = u64::from(track);
    let total64 = u64::from(total);
    let min_len = minimum.max(1).min(track);
    let max_len = (track / THUMB_MAX_DIV).max(min_len);
    let len = ((track64 * u64::from(visible)) / total64)
        .clamp(u64::from(min_len), u64::from(max_len)) as u16;
    let room = track - len;
    // total > track was just checked, so this is strictly positive.
    let scrolled = total64 - u64::from(visible);
    Some(Geometry {
        len,
        room,
        scrolled,
    })
}

/// A track position turned back into an `above`: the inverse of the `start`
/// half of [`thumb`]'s arithmetic. Shared by [`grab`]'s off-thumb jump and
/// [`drag`], so a jump and the drag that follows it agree on where it landed.
fn above_for(start: u16, room: u16, scrolled: u64) -> u32 {
    if room == 0 {
        // Only one position exists; nothing to round towards.
        return 0;
    }
    ((u64::from(start) * scrolled + u64::from(room) / 2) / u64::from(room)) as u32
}

/// Where a thumb held at `grip` rows below its top would sit if the pointer
/// were at `y`, clamped to the track -- dragging past either end pins the
/// thumb to it rather than losing the grab.
fn start_for(track: Rect, room: u16, grip: u16, y: u16) -> u16 {
    let raw = i32::from(y) - i32::from(track.y) - i32::from(grip);
    raw.clamp(0, i32::from(room)) as u16
}

/// The pure geometry: how long a thumb is and where it sits, given how many
/// rows the track has, how many rows the content has, and how many of those
/// rows are above the viewport.
///
/// `None` when there is nothing to scroll -- the content fits in the track,
/// or there is no track to draw on -- which callers take as "do not draw a
/// scrollbar" rather than a zero-length one.
pub fn thumb(track: u16, total: u32, above: u32) -> Option<Thumb> {
    thumb_visible(track, total, above, u32::from(track), 1)
}
fn thumb_visible(track: u16, total: u32, above: u32, visible: u32, minimum: u16) -> Option<Thumb> {
    let g = geometry_visible(track, total, visible, minimum)?;
    let above = std::cmp::min(u64::from(above), g.scrolled);
    // Rounded to the nearest row rather than truncated, so that `above ==
    // scrolled` (the view scrolled all the way) lands the thumb flush with
    // the end of the track instead of one short of it.
    let start = ((above * u64::from(g.room) + g.scrolled / 2) / g.scrolled) as u16;
    let start = start.min(g.room);

    Some(Thumb { start, len: g.len })
}

/// A fixed-row list: `len` items, each one row, scrolled so that `scroll` of
/// them are above the viewport.
pub fn rows(scroll: usize, len: usize, height: u16) -> Option<Thumb> {
    thumb(height, len as u32, scroll as u32)
}

/// The `(total, above)` a variable-height list scrolled through
/// [`crate::vlist::VirtualList`] would hand to [`thumb`], exposed on its own
/// so a caller can record them for [`Scrollbars`] without recomputing them
/// from a second walk of `heights`.
///
/// `heights` is indexed the same way the list itself is walked -- item `i`'s
/// row count, whatever produced the rows `list.visible` handed back. `above`
/// is the rows of every item before the first visible one, plus that item's
/// own `skip` for the case where the viewport starts partway through it --
/// exactly as starcord's chat panel used to compute it by hand.
///
/// `None` for an empty list, which has no rows to measure.
pub fn virtual_extent(
    list: &crate::vlist::VirtualList,
    body: Rect,
    heights: &[u16],
    len: usize,
) -> Option<(u32, u32)> {
    if len == 0 {
        return None;
    }
    let total: u32 = heights.iter().map(|h| u32::from(*h)).sum();
    let get = |i: usize| heights.get(i).copied().unwrap_or(0);
    let visible = list.visible(body, get, len);
    let first = visible.first().map(|v| v.index).unwrap_or(0);
    let skip = visible.first().map(|v| v.skip).unwrap_or(0);
    let above: u32 = heights
        .iter()
        .take(first)
        .map(|h| u32::from(*h))
        .sum::<u32>()
        + u32::from(skip);
    Some((total, above))
}

/// A variable-height list scrolled through [`crate::vlist::VirtualList`].
/// See [`virtual_extent`] for how `total` and `above` are derived.
pub fn virtual_list(
    list: &crate::vlist::VirtualList,
    body: Rect,
    heights: &[u16],
    len: usize,
) -> Option<Thumb> {
    virtual_extent(list, body, heights, len)
        .and_then(|(total, above)| thumb(body.height, total, above))
}

/// The index of the item, counted from the top of `heights`, that contains
/// row `above` -- the inverse of summing heights, for a caller that has
/// dragged the thumb to a new `above` and needs `VirtualList::scroll_to` to
/// follow it. `heights.len() - 1` at most, so a position past the last item
/// (past-the-end `above` values are common; [`thumb`] clamps them rather
/// than rejecting them) lands on the last item rather than off the end.
/// `0` for an empty slice, which has no item to land on.
pub fn index_at(heights: &[u16], above: u32) -> usize {
    let mut seen: u32 = 0;
    for (i, h) in heights.iter().enumerate() {
        seen += u32::from(*h);
        if above < seen {
            return i;
        }
    }
    heights.len().saturating_sub(1)
}

/// The one-column track over the rows `list` occupies. Cell presentation
/// uses the right border; padded native chrome uses the gutter just inside it.
/// Drawing and pointer capture share this rectangle.
///
/// An empty rect when the panel is too narrow to have a right border of its
/// own to draw on.
pub fn track(outer: Rect, list: Rect) -> Rect {
    let inset = if super::frame::padded() { 2 } else { 1 };
    if outer.width <= inset {
        return Rect::default();
    }
    Rect {
        x: outer.x + outer.width - inset,
        y: list.y,
        width: 1,
        height: list.height,
    }
}

/// The same track, at a column that is a divider between list columns rather
/// than the panel's border.
pub fn track_at(x: u16, list: Rect) -> Rect {
    Rect {
        x,
        y: list.y,
        width: 1,
        height: list.height,
    }
}

/// Draw the thumb on its track, and nothing else -- cells outside it are left
/// holding whatever border glyph was already there, which is what makes this
/// safe to call after the border itself is drawn rather than before.
///
/// A no-op for `None`, or for a track with no rows or no column to draw on.
pub fn render(track: Rect, buf: &mut Buffer, t: &Theme, thumb: Option<Thumb>) {
    let Some(thumb) = thumb else {
        return;
    };
    if track.width == 0 || track.height == 0 {
        return;
    }
    let style = Style::default().fg(super::rgb(t.accent));
    let bottom = track.y + track.height;
    for i in 0..thumb.len {
        let y = track.y + thumb.start + i;
        if y < bottom {
            buf.set_string(track.x, y, GLYPH, style);
        }
    }
}

/// Where `y` falls along the track, `0.0` at its top row and `1.0` at its
/// bottom one, clamped to that range for a click or drag that lands outside
/// it. A track one row tall has nowhere to express a fraction, so it is
/// `0.0` throughout.
pub fn fraction_at(track: Rect, y: u16) -> f32 {
    if track.height <= 1 {
        return 0.0;
    }
    let top = track.y;
    let bottom = track.y + track.height - 1;
    let y = y.clamp(top, bottom);
    f32::from(y - top) / f32::from(bottom - top)
}

/// A thumb the pointer has taken hold of: what a later [`drag`] needs to
/// answer, kept around for however many frames the button stays down. `track`
/// and `total` are the ones the press landed on, fixed for the life of the
/// grab even if the list they describe changes shape mid-drag; `grip` is how
/// many rows below the thumb's top the press landed, so the thumb does not
/// jump to be centred under the pointer the moment it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grab {
    track: Rect,
    total: u32,
    grip: u16,
    visible: u32,
    minimum: u16,
}

/// A left press at `(x, y)` against a bar drawn on `track` for content
/// `total` rows tall scrolled `above` rows.
///
/// `None` unless `(x, y)` is on the track's column within its rows and there
/// is a thumb to take hold of -- content that fits needs no scrollbar and
/// gives the mouse nothing to grab either. Otherwise, some `(grab, above)`:
/// a press on the thumb takes hold of the row under the pointer and leaves
/// the position where it was; a press beside it jumps the thumb so its
/// middle lands under the pointer -- `grip` becomes half the thumb's length
/// -- and `above` is that jump applied now, in the same call, rather than
/// waiting for the drag that will usually follow immediately after.
pub fn grab(track: Rect, total: u32, above: u32, x: u16, y: u16) -> Option<(Grab, u32)> {
    grab_visible(track, total, above, x, y, u32::from(track.height), 1)
}
fn grab_visible(
    track: Rect,
    total: u32,
    above: u32,
    x: u16,
    y: u16,
    visible: u32,
    minimum: u16,
) -> Option<(Grab, u32)> {
    if x < track.x || x >= track.x + track.width || y < track.y || y >= track.y + track.height {
        return None;
    }
    let g = geometry_visible(track.height, total, visible, minimum)?;
    let t = thumb_visible(track.height, total, above, visible, minimum)?;
    let row = y - track.y;

    if row >= t.start && row < t.start + t.len {
        let grip = row - t.start;
        return Some((
            Grab {
                track,
                total,
                grip,
                visible,
                minimum,
            },
            above,
        ));
    }

    let grip = t.len / 2;
    let start = start_for(track, g.room, grip, y);
    let above = above_for(start, g.room, g.scrolled);
    Some((
        Grab {
            track,
            total,
            grip,
            visible,
            minimum,
        },
        above,
    ))
}

/// The pointer at row `y` while holding `g`: the `above` the content should
/// show. The inverse of the `start` half of [`thumb`]'s arithmetic -- a drag
/// past either end of the track pins to that end rather than losing the
/// grab, which is what lets a reader fling the thumb to the top or bottom
/// without lining the pointer up with the track's exact last row.
pub fn drag(g: &Grab, y: u16) -> u32 {
    let Some(geo) = geometry_visible(g.track.height, g.total, g.visible, g.minimum) else {
        return 0;
    };
    let start = start_for(g.track, geo.room, g.grip, y);
    above_for(start, geo.room, geo.scrolled)
}

/// The bars drawn this frame, and the one the pointer is holding, keyed by
/// whatever the caller uses to name its lists -- an enum of panel ids, an
/// index, whatever is already at hand when a bar is drawn. One `Scrollbars`
/// per application: call [`Scrollbars::begin_frame`] at the start of every
/// draw, [`Scrollbars::record`] or [`Scrollbars::draw`] once per bar as it is
/// drawn, and the three mouse methods as events arrive. The held grab
/// survives across frames -- and across a frame where its bar was not
/// drawn at all, a list scrolled out of view mid-drag -- until
/// [`Scrollbars::release`] lets it go.
///
/// The one rule a caller has to keep: the `above` a call here hands back is
/// what that same bar must be recorded with next frame. [`Scrollbars::press`]
/// and [`Scrollbars::drag`] return the position to *apply*, not merely to
/// note, and the next [`Scrollbars::record`] or [`Scrollbars::draw`] for that
/// key is where the caller reports it applied. A list that re-derives its
/// scroll from something else every frame -- a selected row, say -- rather
/// than moving that along with the drag, will show the thumb follow the
/// pointer and then snap back the moment the frame redraws; such a caller
/// must move the thing its scroll is derived from, or only re-derive it on
/// keyboard moves and trust the drag the rest of the time. A cursor-driven
/// list that scrolls through [`crate::list::clamp_scroll`] does this with
/// [`crate::list::cursor_into_view`]: `scroll = above; cursor =
/// cursor_into_view(cursor, scroll, height, len)` moves the cursor into
/// whatever the drag just set, so the next frame's `clamp_scroll` leaves it
/// there instead of pulling it back.
pub struct Scrollbars<K: Copy + Eq> {
    drawn: Vec<Bar<K>>,
    held: Option<(K, Grab)>,
}

struct Bar<K> {
    key: K,
    track: Rect,
    total: u32,
    above: u32,
    visible: u32,
    minimum: u16,
}

impl<K: Copy + Eq> Default for Scrollbars<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Eq> Scrollbars<K> {
    pub fn new() -> Self {
        Self {
            drawn: Vec::new(),
            held: None,
        }
    }

    /// Forget last frame's bars. `held` survives -- a grab outlives the
    /// frame it started on, and does not need its bar re-recorded to keep
    /// answering [`Scrollbars::drag`].
    pub fn begin_frame(&mut self) {
        self.drawn.clear();
    }

    /// Remember a bar somebody else drew, or is about to draw with its own
    /// call to [`render`], so the mouse methods know it is there.
    pub fn record(&mut self, key: K, track: Rect, total: u32, above: u32) {
        self.record_viewport(key, track, total, above, u32::from(track.height));
    }

    /// Native rows may be denser than the terminal cells making up the track.
    pub fn record_viewport(&mut self, key: K, track: Rect, total: u32, above: u32, visible: u32) {
        self.record_viewport_with_minimum(key, track, total, above, visible, 1);
    }

    /// Pixel tracks need a grab target larger than one pixel for long lists.
    /// `minimum` uses the track's coordinate units and is bounded by its height.
    /// Drawing, track jumps and held drags all use this same geometry. Existing
    /// terminal callers retain the one-row minimum through `record_viewport`.
    pub fn record_viewport_with_minimum(
        &mut self,
        key: K,
        track: Rect,
        total: u32,
        above: u32,
        visible: u32,
        minimum: u16,
    ) {
        self.drawn.retain(|bar| bar.key != key);
        self.drawn.push(Bar {
            key,
            track,
            total,
            above,
            visible,
            minimum,
        });
    }

    /// Record and draw in one call: [`thumb`] of `track.height`, `total` and
    /// `above`, rendered on `track`.
    pub fn draw(
        &mut self,
        key: K,
        track: Rect,
        buf: &mut Buffer,
        t: &Theme,
        total: u32,
        above: u32,
    ) {
        self.record(key, track, total, above);
        render(track, buf, t, thumb(track.height, total, above));
    }

    /// Visible track and thumb geometry, shared with native pixel presentation.
    pub fn visible(&self) -> impl Iterator<Item = (Rect, Thumb)> + '_ {
        self.drawn.iter().filter_map(|bar| {
            thumb_visible(
                bar.track.height,
                bar.total,
                bar.above,
                bar.visible,
                bar.minimum,
            )
            .map(|thumb| (bar.track, thumb))
        })
    }

    /// The track recorded for `key` this frame, if any.
    pub fn track_of(&self, key: K) -> Option<Rect> {
        self.drawn.iter().find(|b| b.key == key).map(|b| b.track)
    }

    /// A left press. `Some((key, above))` when it landed on a bar recorded
    /// this frame: that bar's grab is now held, and `above` is the position
    /// to apply -- unchanged if the press was on the thumb, the jumped-to
    /// position otherwise. `None` when it landed on none of them, meaning
    /// the press is not this scrollbar's to answer.
    pub fn press(&mut self, x: u16, y: u16) -> Option<(K, u32)> {
        for bar in &self.drawn {
            if let Some((g, above)) = grab_visible(
                bar.track,
                bar.total,
                bar.above,
                x,
                y,
                bar.visible,
                bar.minimum,
            ) {
                let key = bar.key;
                self.held = Some((key, g));
                return Some((key, above));
            }
        }
        None
    }

    /// A drag event while holding a grab: the key it belongs to and the
    /// `above` to apply. `None` when nothing is held, so a drag that starts
    /// outside any bar's thumb is silently not this scrollbar's concern.
    pub fn drag(&mut self, y: u16) -> Option<(K, u32)> {
        let (key, g) = self.held.as_ref()?;
        Some((*key, drag(g, y)))
    }

    /// The button came up. Whether something was held, so a caller can tell
    /// a released drag from a click that never touched a bar.
    pub fn release(&mut self) -> bool {
        self.held.take().is_some()
    }

    /// The key of the bar currently held, if any.
    pub fn held(&self) -> Option<K> {
        self.held.as_ref().map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vlist::VirtualList;

    #[test]
    fn compact_native_rows_use_content_capacity_for_drag_limits() {
        let mut bars = Scrollbars::<u8>::new();
        let track = Rect::new(5, 10, 1, 10);
        bars.record_viewport(1, track, 40, 0, 16);
        assert_eq!(bars.press(5, 10).unwrap(), (1, 0));
        assert_eq!(bars.drag(100).unwrap(), (1, 24));
        bars.release();
        bars.record_viewport(1, track, 12, 0, 16);
        assert_eq!(bars.visible().count(), 0);
        assert!(bars.press(5, 10).is_none());
    }

    #[test]
    fn native_track_is_inside_border_and_uses_the_same_drag_target() {
        let outer = Rect::new(10, 5, 40, 20);
        let list = Rect::new(12, 9, 36, 12);
        assert_eq!(track(outer, list).x, outer.right() - 1);
        {
            let _scope = super::super::frame::padding_scope(true);
            let inside = track(outer, list);
            assert_eq!(inside.x, list.right());
            assert_eq!(inside.right(), outer.right() - 1);
            assert!(grab(inside, 100, 0, inside.x, inside.y).is_some());
            assert!(grab(inside, 100, 0, outer.right() - 1, inside.y).is_none());
            assert_eq!(track(Rect::new(0, 0, 2, 3), list), Rect::default());
        }
        assert_eq!(track(outer, list).x, outer.right() - 1);
    }

    #[test]
    fn a_list_that_fits_has_no_thumb() {
        assert_eq!(thumb(10, 10, 0), None, "total == track");
        assert_eq!(thumb(10, 5, 0), None, "total < track");
        assert_eq!(thumb(0, 100, 0), None, "no track at all");
    }

    #[test]
    fn the_thumb_is_never_longer_than_a_quarter_of_the_track() {
        for track in 1..=60u16 {
            let mut total = u32::from(track) + 1;
            let max_total = u32::from(track) * 10;
            while total <= max_total {
                let t = thumb(track, total, 0).unwrap();
                let cap = std::cmp::max(1, track / THUMB_MAX_DIV);
                assert!(
                    t.len <= cap,
                    "track {track} total {total}: len {} > cap {cap}",
                    t.len
                );
                assert!(t.len >= 1, "track {track} total {total}: len is zero");
                total += std::cmp::max(1, track / 3) as u32;
            }
        }
    }

    #[test]
    fn the_top_is_the_top_and_the_end_reaches_the_end() {
        for (track, total) in [(10u16, 50u32), (5, 6), (100, 1000), (3, 100), (40, 41)] {
            let at_top = thumb(track, total, 0).unwrap();
            assert_eq!(at_top.start, 0, "track {track} total {total}: not at top");

            let scrolled = total - u32::from(track);
            let at_end = thumb(track, total, scrolled).unwrap();
            assert_eq!(
                at_end.start + at_end.len,
                track,
                "track {track} total {total}: does not reach the end"
            );

            // Past the natural range clamps to the same place as exactly at it.
            let past_end = thumb(track, total, scrolled * 3 + 7).unwrap();
            assert_eq!(past_end.start, at_end.start);
        }
    }

    #[test]
    fn the_thumb_moves_monotonically_and_never_leaves_the_track() {
        for (track, total) in [(20u16, 200u32), (7, 8), (50, 5000)] {
            let scrolled = total - u32::from(track);
            let mut last_start = 0u16;
            let mut above = 0u32;
            let step = std::cmp::max(1, scrolled / 37);
            loop {
                let t = thumb(track, total, above).unwrap();
                assert!(
                    t.start >= last_start,
                    "track {track} total {total} above {above}: start went backwards"
                );
                assert!(
                    t.start + t.len <= track,
                    "track {track} total {total} above {above}: thumb leaves the track"
                );
                last_start = t.start;
                if above >= scrolled {
                    break;
                }
                above = std::cmp::min(above + step, scrolled);
            }
        }
    }

    #[test]
    fn a_three_row_track_still_shows_position() {
        let at_top = thumb(3, 100, 0).unwrap();
        assert_eq!(at_top, Thumb { start: 0, len: 1 });

        let at_end = thumb(3, 100, 97).unwrap();
        assert_eq!(at_end, Thumb { start: 2, len: 1 });
    }

    #[test]
    fn render_touches_only_the_thumb_rows_on_the_track_column() {
        let area = Rect::new(0, 0, 3, 6);
        let mut buf = Buffer::empty(area);
        for y in 0..6 {
            buf.set_string(0, y, "X", Style::default());
            buf.set_string(1, y, "\u{2551}", Style::default());
        }
        let theme = super::super::test_theme("cosmic");
        let track = Rect::new(1, 0, 1, 6);
        render(track, &mut buf, &theme, Some(Thumb { start: 2, len: 2 }));

        for y in 0..6u16 {
            let sym = buf[(1, y)].symbol();
            if (2..4).contains(&y) {
                assert_eq!(sym, GLYPH, "row {y} should be the thumb");
            } else {
                assert_eq!(sym, "\u{2551}", "row {y} should be untouched");
            }
            assert_eq!(buf[(0, y)].symbol(), "X", "the column to the left moved");
        }
    }

    #[test]
    fn render_is_a_no_op_for_none_or_an_empty_track() {
        let area = Rect::new(0, 0, 3, 6);
        let mut buf = Buffer::empty(area);
        for y in 0..6 {
            buf.set_string(1, y, "\u{2551}", Style::default());
        }
        let before = format!("{buf:?}");
        let theme = super::super::test_theme("cosmic");
        render(Rect::new(1, 0, 1, 6), &mut buf, &theme, None);
        render(
            Rect::new(1, 0, 0, 6),
            &mut buf,
            &theme,
            Some(Thumb { start: 0, len: 1 }),
        );
        assert_eq!(format!("{buf:?}"), before);
    }

    #[test]
    fn fraction_at_is_zero_at_the_top_and_one_at_the_bottom() {
        let track = Rect::new(0, 5, 1, 10);
        assert_eq!(fraction_at(track, 5), 0.0);
        assert_eq!(fraction_at(track, 14), 1.0);
        // Clamped outside the track.
        assert_eq!(fraction_at(track, 0), 0.0);
        assert_eq!(fraction_at(track, 100), 1.0);
        // Somewhere in the middle.
        assert!((fraction_at(track, 9) - 4.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn fraction_at_is_zero_throughout_a_one_row_track() {
        let track = Rect::new(0, 3, 1, 1);
        assert_eq!(fraction_at(track, 3), 0.0);
        assert_eq!(fraction_at(track, 0), 0.0);
        assert_eq!(fraction_at(track, 50), 0.0);
    }

    #[test]
    fn virtual_list_agrees_with_rows_for_unit_heights() {
        let heights = vec![1u16; 50];
        let len = heights.len();
        let body = Rect::new(0, 0, 10, 8);
        let get = |i: usize| heights.get(i).copied().unwrap_or(0);

        for anchor in [0usize, 5, 20, 42] {
            let list = VirtualList::at(anchor);
            let visible = list.visible(body, get, len);
            let first = visible.first().map(|v| v.index).unwrap_or(0);
            let skip = visible.first().map(|v| v.skip).unwrap_or(0);
            let above = first + usize::from(skip);
            assert_eq!(
                virtual_list(&list, body, &heights, len),
                rows(above, len, body.height),
                "anchor {anchor}"
            );
        }

        // Stuck to the end, which is where a fresh list starts.
        let list = VirtualList::new();
        let visible = list.visible(body, get, len);
        let first = visible.first().map(|v| v.index).unwrap_or(0);
        assert_eq!(
            virtual_list(&list, body, &heights, len),
            rows(first, len, body.height),
            "stuck to the end"
        );
    }

    #[test]
    fn virtual_list_matches_virtual_extent() {
        let heights = vec![3u16, 1, 2, 4, 1, 5, 2];
        let len = heights.len();
        let body = Rect::new(0, 0, 10, 6);
        for anchor in [0usize, 2, 4, 6] {
            let list = VirtualList::at(anchor);
            let expected = virtual_extent(&list, body, &heights, len)
                .and_then(|(total, above)| thumb(body.height, total, above));
            assert_eq!(
                virtual_list(&list, body, &heights, len),
                expected,
                "anchor {anchor}"
            );
        }
        assert_eq!(virtual_extent(&VirtualList::new(), body, &heights, 0), None);
        assert_eq!(virtual_list(&VirtualList::new(), body, &heights, 0), None);
    }

    #[test]
    fn a_press_on_the_thumb_keeps_the_position_and_a_press_beside_it_jumps() {
        // track 20 rows, total 100, above 0 -- thumb is len 4 at start 0.
        let track = Rect::new(5, 2, 1, 20);

        let (on_thumb, above) = grab(track, 100, 0, 5, 2).expect("row 0 is on the thumb");
        assert_eq!(above, 0, "a press on the thumb does not move the position");
        assert_eq!(
            drag(&on_thumb, 2),
            0,
            "dragging it back to where it was grabbed stays put"
        );

        let (beside, jumped) =
            grab(track, 100, 0, 5, 21).expect("the last track row is on the bar");
        assert!(
            jumped > 50,
            "a press near the bottom of the track should jump close to the end, got {jumped}"
        );

        assert_eq!(
            drag(&beside, 2),
            0,
            "dragging the jumped-to grab up to the top reaches the start"
        );
    }

    #[test]
    fn dragging_to_the_ends_reaches_the_ends() {
        let track = Rect::new(0, 5, 1, 20);
        let (g, _) = grab(track, 100, 0, 0, 5).expect("the top row is on the thumb");

        assert_eq!(
            drag(&g, 200),
            100 - 20,
            "a drag below the track pins to the end"
        );
        assert_eq!(drag(&g, 0), 0, "a drag above the track pins to the start");
    }

    #[test]
    fn drag_is_the_inverse_of_thumb() {
        for (track, total, above) in [
            (20u16, 100u32, 30u32),
            (7, 50, 10),
            (50, 500, 200),
            (3, 10, 4),
        ] {
            let geo = geometry(track, total).unwrap();
            let track_rect = Rect::new(2, 3, 1, track);
            let t = thumb(track, total, above).unwrap();
            let y = track_rect.y + t.start;

            let (g, unchanged) = grab(track_rect, total, above, track_rect.x, y)
                .unwrap_or_else(|| panic!("track {track} total {total} above {above}: the thumb's own top missed the thumb"));
            assert_eq!(
                unchanged, above,
                "track {track} total {total}: a press on the thumb moves the position"
            );

            let back = drag(&g, y);
            // The finest a drag can distinguish is one row of track, which
            // stands for this many rows of content.
            let granularity = if geo.room == 0 {
                1
            } else {
                geo.scrolled.div_ceil(u64::from(geo.room))
            } as i64;
            assert!(
                (i64::from(back) - i64::from(above)).abs() <= granularity,
                "track {track} total {total} above {above}: drag back gave {back}"
            );
            assert_eq!(
                thumb(track, total, back).unwrap().start,
                t.start,
                "track {track} total {total} above {above}: the same row does not land back on the same start"
            );
        }
    }

    #[test]
    fn a_press_off_the_track_is_none() {
        let track = Rect::new(5, 2, 1, 20);
        assert_eq!(grab(track, 100, 0, 4, 5), None, "wrong column");
        assert_eq!(grab(track, 100, 0, 6, 5), None, "wrong column");
        assert_eq!(grab(track, 100, 0, 5, 1), None, "above the track");
        assert_eq!(grab(track, 100, 0, 5, 22), None, "below the track");
        // A track with nothing to scroll has no thumb to take hold of either.
        assert_eq!(grab(track, 10, 0, 5, 5), None, "content fits, no thumb");
    }

    #[test]
    fn index_at_finds_the_item_containing_the_row() {
        let heights = [3u16, 1, 2];
        assert_eq!(index_at(&heights, 0), 0);
        assert_eq!(index_at(&heights, 1), 0);
        assert_eq!(index_at(&heights, 2), 0);
        assert_eq!(index_at(&heights, 3), 1);
        assert_eq!(index_at(&heights, 4), 2);
        assert_eq!(index_at(&heights, 5), 2);
        assert_eq!(
            index_at(&heights, 99),
            2,
            "past the end lands on the last item"
        );
        assert_eq!(index_at(&[], 0), 0, "nothing to land on");
    }

    #[test]
    fn pixel_minimum_preserves_grab_and_full_scroll_range_at_both_densities() {
        for density in [1, 2] {
            let track = Rect::new(100, 50, 16 * density, 400 * density);
            let minimum = 24 * density;
            let mut bars = Scrollbars::new();
            for total in [30_000, 3_000_000] {
                bars.record_viewport_with_minimum((), track, total, 0, 20, minimum);
                let (_, thumb) = bars.visible().next().unwrap();
                assert_eq!(thumb.len, minimum);
                let y = track.y + thumb.len - 1;
                assert_eq!(
                    bars.press(track.x + 2, y),
                    Some(((), 0)),
                    "the bottom of the visible thumb is still a grab, not a jump"
                );
                assert_eq!(
                    bars.drag(track.y + track.height + minimum),
                    Some(((), total - 20))
                );
                assert_eq!(bars.drag(0), Some(((), 0)));
                bars.release();
                bars.record_viewport_with_minimum((), track, total, total - 20, 20, minimum);
                let (_, thumb) = bars.visible().next().unwrap();
                assert_eq!(thumb.start + thumb.len, track.height);
            }
            bars.record_viewport_with_minimum((), Rect::new(0, 0, 10, 8), 100, 0, 4, minimum);
            assert_eq!(bars.visible().next().unwrap().1.len, 8);
            bars.record_viewport_with_minimum((), track, 20, 0, 20, minimum);
            assert!(bars.visible().next().is_none());
        }
    }

    #[test]
    fn the_registry_holds_across_frames_and_lets_go_on_release() {
        let mut bars: Scrollbars<&str> = Scrollbars::new();
        bars.begin_frame();
        bars.record("first", Rect::new(0, 0, 1, 10), 50, 0);
        bars.record("second", Rect::new(2, 0, 1, 10), 50, 0);

        // Second bar's thumb: track 10, total 50, above 0 -- press its row 0.
        let pressed = bars.press(2, 0);
        assert_eq!(pressed.map(|(k, _)| k), Some("second"));
        assert_eq!(bars.held(), Some("second"));

        bars.begin_frame();
        // No bars recorded this frame -- the grab still answers.
        let dragged = bars.drag(9);
        assert_eq!(dragged.map(|(k, _)| k), Some("second"));

        assert!(bars.release());
        assert_eq!(bars.held(), None);
        assert_eq!(bars.drag(9), None, "nothing held after release");
    }

    #[test]
    fn a_frame_without_the_bar_still_answers_the_held_drag() {
        let mut bars: Scrollbars<u8> = Scrollbars::new();
        bars.begin_frame();
        let track = Rect::new(0, 0, 1, 20);
        bars.draw(
            1,
            track,
            &mut Buffer::empty(track),
            &super::super::test_theme("cosmic"),
            100,
            0,
        );

        let (key, above) = bars.press(0, 0).expect("row 0 is the thumb's top");
        assert_eq!(key, 1);
        assert_eq!(above, 0);

        bars.begin_frame(); // the caller drew nothing this time
        let (key, above) = bars.drag(19).expect("the grab is still held");
        assert_eq!(key, 1);
        assert_eq!(above, 100 - 20);
    }
}
