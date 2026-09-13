//! Scrolling a list whose items are not all the same height, from the bottom.
//!
//! A message list has two properties an ordinary scroll offset cannot express.
//! Items have different heights, and the heights are not known until they have
//! been wrapped, so a single "row 4218 of 9930" offset would mean measuring
//! every item above the viewport on every frame. And the list grows at the
//! end: a scroll position counted from the top moves under the reader every
//! time somebody sends a message.
//!
//! So the position is an *anchor*: either "stuck to the end", or item `n` with
//! its row `offset` at the top of the viewport. Both are stable when items are
//! appended, and neither costs more than the items actually on screen.
//!
//! Heights arrive through a closure rather than a slice, because the caller
//! has them in a wrap cache and computing one is not free. Nothing here asks
//! for a height it does not put on screen, except when [`VirtualList::
//! scroll_rows`] has to decide whether a scroll ran off the end, and then it
//! stops as soon as it has seen a viewport's worth.

use ratatui::layout::Rect;

/// One item's place on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible {
    /// Index of the item.
    pub index: usize,
    /// Where it is drawn, already clipped to the viewport.
    pub area: Rect,
    /// Rows of the item cut off above `area`, non-zero only for the first
    /// item and only when the viewport starts part way into it. An item
    /// taller than the whole viewport is scrolled through with this.
    pub skip: u16,
}

/// Where a variable-height list is scrolled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualList {
    anchor: usize,
    offset: u16,
    stick_to_end: bool,
}

impl Default for VirtualList {
    /// Stuck to the end, because that is where a conversation starts.
    fn default() -> Self {
        Self {
            anchor: 0,
            offset: 0,
            stick_to_end: true,
        }
    }
}

impl VirtualList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Anchored with item `index` at the top of the viewport.
    pub fn at(index: usize) -> Self {
        Self {
            anchor: index,
            offset: 0,
            stick_to_end: false,
        }
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    pub fn offset(&self) -> u16 {
        self.offset
    }

    /// Is the view pinned to the last item? The status bar says "3 new" when
    /// it is not.
    pub fn is_at_end(&self) -> bool {
        self.stick_to_end
    }

    /// Pin to the end again, and stay there as items arrive.
    pub fn to_end(&mut self) {
        self.stick_to_end = true;
        self.offset = 0;
    }

    /// Put `index` at the top of the viewport.
    pub fn scroll_to(&mut self, index: usize) {
        self.anchor = index;
        self.offset = 0;
        self.stick_to_end = false;
    }

    /// The items on screen, in order, with the rows each of them gets.
    ///
    /// When there is less content than viewport the list is drawn from the
    /// top and the space below it is left empty, in both modes. Growing
    /// downwards as messages arrive reads better than a short block that
    /// creeps upwards, and the caller that wants the other behaviour can pad
    /// the rect it passes in.
    pub fn visible(&self, area: Rect, heights: impl Fn(usize) -> u16, len: usize) -> Vec<Visible> {
        if area.height == 0 || area.width == 0 || len == 0 {
            return Vec::new();
        }
        let (mut index, skip) = self.top(area.height, &heights, len);
        let mut out = Vec::new();
        let mut y = area.y;
        let bottom = area.y + area.height;
        let mut skip = skip;

        while index < len && y < bottom {
            let h = heights(index).saturating_sub(skip);
            if h == 0 {
                // A zero-height item takes no rows and no rect; without this
                // the walk would still terminate, but on the viewport rather
                // than on the list.
                index += 1;
                skip = 0;
                continue;
            }
            let h = h.min(bottom - y);
            out.push(Visible {
                index,
                area: Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: h,
                },
                skip,
            });
            y += h;
            index += 1;
            skip = 0;
        }
        out
    }

    /// Which item and which of its rows is at `y`, the inverse of
    /// [`VirtualList::visible`].
    pub fn hit(
        &self,
        area: Rect,
        heights: impl Fn(usize) -> u16,
        len: usize,
        y: u16,
    ) -> Option<(usize, u16)> {
        self.visible(area, heights, len)
            .into_iter()
            .find(|v| y >= v.area.y && y < v.area.y + v.area.height)
            .map(|v| (v.index, y - v.area.y + v.skip))
    }

    /// Scroll by rows: negative towards the start, positive towards the end.
    ///
    /// The first scroll away from the end converts the position into an
    /// explicit anchor, so appended items no longer move the view; scrolling
    /// back down to within a viewport of the last item returns it to sticking,
    /// so the list follows the conversation again without the caller having to
    /// notice.
    ///
    /// `area` is the viewport the scroll happens in -- only its height is
    /// used, but passing the same rect as `visible` is harder to get wrong
    /// than passing a number that has to match it.
    pub fn scroll_rows(
        &mut self,
        delta: i32,
        area: Rect,
        heights: impl Fn(usize) -> u16,
        len: usize,
    ) {
        if len == 0 || area.height == 0 || delta == 0 {
            return;
        }
        let (mut index, mut offset) = self.top(area.height, &heights, len);

        if delta < 0 {
            let mut up = delta.unsigned_abs();
            while up > 0 {
                if u32::from(offset) >= up {
                    offset -= up as u16;
                    break;
                }
                up -= u32::from(offset);
                if index == 0 {
                    offset = 0;
                    break;
                }
                index -= 1;
                offset = heights(index);
            }
            self.anchor = index;
            self.offset = offset;
            self.stick_to_end = false;
            return;
        }

        let mut down = delta as u32;
        while down > 0 {
            let room = u32::from(heights(index).saturating_sub(offset));
            if down < room {
                offset += down as u16;
                break;
            }
            down -= room;
            if index + 1 >= len {
                self.to_end();
                return;
            }
            index += 1;
            offset = 0;
        }

        // Past the end means the last item's bottom has come up inside the
        // viewport; from there, follow the end.
        if rows_from(index, offset, &heights, len, area.height) <= area.height {
            self.to_end();
        } else {
            self.anchor = index;
            self.offset = offset;
            self.stick_to_end = false;
        }
    }

    /// The item and row at the top of the viewport, resolving `stick_to_end`
    /// against the heights it would take to fill the viewport from the end.
    fn top(&self, height: u16, heights: &impl Fn(usize) -> u16, len: usize) -> (usize, u16) {
        if !self.stick_to_end {
            let index = self.anchor.min(len - 1);
            let offset = if index == self.anchor { self.offset } else { 0 };
            return (index, offset.min(heights(index)));
        }
        let mut index = len - 1;
        let mut acc = 0u32;
        loop {
            let h = u32::from(heights(index));
            if acc + h >= u32::from(height) {
                // This item is the top one, cut off by whatever does not fit.
                return (index, (acc + h - u32::from(height)) as u16);
            }
            acc += h;
            if index == 0 {
                return (0, 0);
            }
            index -= 1;
        }
    }
}

/// Rows from `(index, offset)` to the end of the list, giving up once `enough`
/// has been counted: the caller only ever wants to compare it with a viewport.
fn rows_from(
    index: usize,
    offset: u16,
    heights: &impl Fn(usize) -> u16,
    len: usize,
    enough: u16,
) -> u16 {
    let mut total = u32::from(heights(index).saturating_sub(offset));
    let mut i = index + 1;
    while i < len && total <= u32::from(enough) {
        total += u32::from(heights(i));
        i += 1;
    }
    total.min(u32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: Rect = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 10,
    };

    /// Heights 1, 2, 3, 1, 2, 3, .. so that no two arrangements look alike.
    fn mixed(i: usize) -> u16 {
        [1u16, 2, 3][i % 3]
    }

    fn flat(_: usize) -> u16 {
        2
    }

    #[test]
    fn a_new_list_shows_the_end() {
        // 30 items of 1, 2, 3 repeating is 60 rows. The last ten are items
        // 25..=29 -- 2 + 1 + 3 + 2 + 3 -- with the first of them a row short
        // of fitting, so one of its two rows is cut off above the viewport.
        let l = VirtualList::new();
        let v = l.visible(VIEW, mixed, 30);
        assert_eq!(v.first().unwrap().index, 25);
        assert_eq!(v.first().unwrap().skip, 1);
        assert_eq!(v.last().unwrap().index, 29);
        let bottom = v.last().unwrap().area;
        assert_eq!(bottom.y + bottom.height, VIEW.height, "flush with the end");
    }

    #[test]
    fn the_rows_of_the_visible_items_tile_the_viewport() {
        let l = VirtualList::new();
        let v = l.visible(VIEW, mixed, 30);
        let mut y = VIEW.y;
        for item in &v {
            assert_eq!(item.area.y, y);
            y += item.area.height;
        }
        assert_eq!(y, VIEW.y + VIEW.height);
    }

    #[test]
    fn a_short_list_sits_at_the_top() {
        let l = VirtualList::new();
        let v = l.visible(VIEW, flat, 3);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].index, 0);
        assert_eq!(v[0].area.y, 0);
        assert_eq!(v[0].skip, 0);
    }

    #[test]
    fn an_empty_list_shows_nothing() {
        assert!(VirtualList::new().visible(VIEW, flat, 0).is_empty());
    }

    #[test]
    fn scrolling_up_leaves_the_end_and_stays_put_when_items_arrive() {
        let mut l = VirtualList::new();
        l.scroll_rows(-4, VIEW, mixed, 30);
        assert!(!l.is_at_end());
        let before = l.visible(VIEW, mixed, 30);

        // Six more messages arrive. Nothing on screen moves.
        let after = l.visible(VIEW, mixed, 36);
        assert_eq!(before, after);
    }

    #[test]
    fn scrolling_up_by_one_row_moves_the_view_by_exactly_one_row() {
        let mut l = VirtualList::new();
        let before = l.visible(VIEW, mixed, 30);
        l.scroll_rows(-1, VIEW, mixed, 30);
        let after = l.visible(VIEW, mixed, 30);
        assert_eq!(after.first().unwrap().index, before.first().unwrap().index);
        assert_eq!(
            after.first().unwrap().skip,
            before.first().unwrap().skip - 1
        );
    }

    #[test]
    fn scrolling_up_past_the_start_stops_at_the_start() {
        let mut l = VirtualList::new();
        l.scroll_rows(-1000, VIEW, mixed, 30);
        let v = l.visible(VIEW, mixed, 30);
        assert_eq!(v.first().unwrap().index, 0);
        assert_eq!(v.first().unwrap().skip, 0);
    }

    #[test]
    fn scrolling_back_down_to_the_end_sticks_again() {
        let mut l = VirtualList::new();
        l.scroll_rows(-7, VIEW, mixed, 30);
        assert!(!l.is_at_end());
        l.scroll_rows(7, VIEW, mixed, 30);
        assert!(l.is_at_end());
    }

    #[test]
    fn scrolling_down_past_the_end_sticks_rather_than_running_off() {
        let mut l = VirtualList::new();
        l.scroll_rows(-20, VIEW, mixed, 30);
        l.scroll_rows(1000, VIEW, mixed, 30);
        assert!(l.is_at_end());
        assert_eq!(
            l.visible(VIEW, mixed, 30),
            VirtualList::new().visible(VIEW, mixed, 30)
        );
    }

    #[test]
    fn an_item_taller_than_the_viewport_is_scrolled_through_by_rows() {
        let tall = |i: usize| if i == 1 { 25 } else { 2 };
        let mut l = VirtualList::at(1);
        for expected in 0..15 {
            let v = l.visible(VIEW, tall, 3);
            assert_eq!(v[0].index, 1);
            assert_eq!(v[0].skip, expected);
            assert_eq!(v[0].area.height, 10);
            l.scroll_rows(1, VIEW, tall, 3);
        }
    }

    #[test]
    fn hit_testing_is_the_inverse_of_visible() {
        let l = VirtualList::new();
        let v = l.visible(VIEW, mixed, 30);
        for item in &v {
            for row in 0..item.area.height {
                let y = item.area.y + row;
                assert_eq!(
                    l.hit(VIEW, mixed, 30, y),
                    Some((item.index, row + item.skip)),
                    "row {y}"
                );
            }
        }
    }

    #[test]
    fn a_row_below_the_content_hits_nothing() {
        let l = VirtualList::new();
        assert_eq!(l.hit(VIEW, flat, 3, 9), None);
        assert_eq!(l.hit(VIEW, flat, 3, 2), Some((1, 0)));
    }

    #[test]
    fn scrolling_is_reversible_within_the_list() {
        let mut l = VirtualList::at(10);
        let before = l.visible(VIEW, mixed, 30);
        l.scroll_rows(-5, VIEW, mixed, 30);
        l.scroll_rows(5, VIEW, mixed, 30);
        assert_eq!(l.visible(VIEW, mixed, 30), before);
    }

    #[test]
    fn zero_height_items_do_not_stall_the_walk() {
        // A message whose height has not been measured yet is zero rows; the
        // walk must step over it rather than filling the viewport with it.
        let sparse = |i: usize| if i % 2 == 0 { 0 } else { 1 };
        let l = VirtualList::at(0);
        let v = l.visible(VIEW, sparse, 40);
        assert_eq!(v.len(), 10);
        assert!(v.iter().all(|x| x.index % 2 == 1));
    }

    #[test]
    fn an_anchor_past_the_end_falls_back_to_the_last_item() {
        let l = VirtualList::at(99);
        let v = l.visible(VIEW, flat, 4);
        assert_eq!(v.first().unwrap().index, 3);
    }
}
