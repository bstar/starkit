//! Scrolling a list of rows.
//!
//! A `VirtualList` -- the bottom-anchored, variable-height scroller a message
//! view needs -- belongs beside this, in `src/list/`. It is not here yet; what
//! is here is the arithmetic every fixed-height list in both applications was
//! writing out for itself.

/// Keep the cursor visible.
pub fn clamp_scroll(cursor: usize, scroll: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    if cursor < scroll {
        cursor
    } else if cursor >= scroll + height {
        cursor + 1 - height
    } else {
        scroll
    }
}

/// The inverse move: `scroll` was just set from outside -- a dragged
/// scrollbar, a jump -- and the cursor has to follow it into the new
/// viewport, or `clamp_scroll` would drag `scroll` right back to wherever the
/// cursor still is. The nearest row inside `scroll..scroll + height`, so a
/// cursor already in view is left alone rather than recentred.
///
/// `0` when there is nothing to put a cursor on, `height == 0` or `len == 0`.
pub fn cursor_into_view(cursor: usize, scroll: usize, height: usize, len: usize) -> usize {
    if height == 0 || len == 0 {
        return 0;
    }
    let last = len - 1;
    if cursor < scroll {
        scroll.min(last)
    } else if cursor >= scroll + height {
        (scroll + height - 1).min(last)
    } else {
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_follows_the_cursor() {
        assert_eq!(clamp_scroll(0, 0, 8), 0);
        assert_eq!(clamp_scroll(9, 0, 8), 2);
        assert_eq!(clamp_scroll(1, 5, 8), 1);
        assert_eq!(clamp_scroll(3, 0, 0), 0);
    }

    #[test]
    fn the_cursor_follows_a_scroll_set_from_outside() {
        // Above the window: pulled down to its top.
        assert_eq!(cursor_into_view(0, 5, 8, 100), 5);
        // Below the window: pulled up to its bottom.
        assert_eq!(cursor_into_view(50, 5, 8, 100), 12);
        // Already inside: left alone.
        assert_eq!(cursor_into_view(7, 5, 8, 100), 7);
        // Bottom of the window past the end of a short list: pinned to it.
        assert_eq!(cursor_into_view(20, 5, 8, 10), 9);
        // Nothing to put a cursor on.
        assert_eq!(cursor_into_view(3, 0, 8, 0), 0);
        assert_eq!(cursor_into_view(3, 0, 0, 8), 0);
    }

    #[test]
    fn cursor_into_view_round_trips_through_clamp_scroll() {
        let len = 30;
        for height in 1..len {
            for scroll in 0..=(len - height) {
                for cursor in 0..len {
                    let c = cursor_into_view(cursor, scroll, height, len);
                    assert_eq!(
                        clamp_scroll(c, scroll, height),
                        scroll,
                        "height {height} scroll {scroll} cursor {cursor}: landed on {c}"
                    );
                }
            }
        }
    }
}
