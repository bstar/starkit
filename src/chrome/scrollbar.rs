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

/// The pure geometry: how long a thumb is and where it sits, given how many
/// rows the track has, how many rows the content has, and how many of those
/// rows are above the viewport.
///
/// `None` when there is nothing to scroll -- the content fits in the track,
/// or there is no track to draw on -- which callers take as "do not draw a
/// scrollbar" rather than a zero-length one.
pub fn thumb(track: u16, total: u32, above: u32) -> Option<Thumb> {
    if track == 0 || u64::from(total) <= u64::from(track) {
        return None;
    }
    let track64 = u64::from(track);
    let total64 = u64::from(total);
    let max_len = std::cmp::max(1, track / THUMB_MAX_DIV);
    let len = ((track64 * track64) / total64).clamp(1, u64::from(max_len)) as u16;

    let room = track - len;
    // total > track was just checked, so this is strictly positive.
    let scrolled = total64 - track64;
    let above = std::cmp::min(u64::from(above), scrolled);
    // Rounded to the nearest row rather than truncated, so that `above ==
    // scrolled` (the view scrolled all the way) lands the thumb flush with
    // the end of the track instead of one short of it.
    let start = ((above * u64::from(room) + scrolled / 2) / scrolled) as u16;
    let start = start.min(room);

    Some(Thumb { start, len })
}

/// A fixed-row list: `len` items, each one row, scrolled so that `scroll` of
/// them are above the viewport.
pub fn rows(scroll: usize, len: usize, height: u16) -> Option<Thumb> {
    thumb(height, len as u32, scroll as u32)
}

/// A variable-height list scrolled through [`crate::vlist::VirtualList`].
///
/// `heights` is indexed the same way the list itself is walked -- item `i`'s
/// row count, whatever produced the rows `list.visible` handed back. `total`
/// and `above` are derived from it exactly as starcord's chat panel used to
/// compute them by hand: `above` is the rows of every item before the first
/// visible one, plus that item's own `skip` for the case where the viewport
/// starts partway through it.
pub fn virtual_list(
    list: &crate::vlist::VirtualList,
    body: Rect,
    heights: &[u16],
    len: usize,
) -> Option<Thumb> {
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
    thumb(body.height, total, above)
}

/// The one-column track on a panel's right border: the last column of
/// `outer`, over the rows `list` occupies.
///
/// An empty rect when the panel is too narrow to have a right border of its
/// own to draw on.
pub fn track(outer: Rect, list: Rect) -> Rect {
    if outer.width < 2 {
        return Rect::default();
    }
    Rect {
        x: outer.x + outer.width - 1,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vlist::VirtualList;

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
}
