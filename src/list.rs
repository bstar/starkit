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
}
