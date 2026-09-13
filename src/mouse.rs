//! Pointer arithmetic, and the one piece of mouse state that is not geometry.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;

/// Is this cell inside the rect?
pub fn hit(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// Two clicks closer together than this, on the same cell, are a double click.
///
/// A terminal reports presses and releases and leaves the pairing to the
/// program, so this is ours to choose. 450 ms is at the slow end of what
/// desktop toolkits use, because the pointer in a terminal is often a trackpad
/// and the target is one cell wide.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(450);

/// Remembers the last click so the next one can say whether it was a double.
///
/// The same cell as well as the same moment: a second click a row away is a
/// new selection, not an activation of the first one.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClickTracker {
    last: Option<(u16, u16, Instant)>,
}

impl ClickTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a click and say whether it completes a double click.
    pub fn click(&mut self, x: u16, y: u16) -> bool {
        self.click_at(x, y, Instant::now())
    }

    /// The same, against a clock the caller supplies, so the window is testable
    /// without sleeping for half a second.
    pub fn click_at(&mut self, x: u16, y: u16, now: Instant) -> bool {
        let double = self.last.is_some_and(|(px, py, at)| {
            px == x && py == y && now.duration_since(at) < DOUBLE_CLICK
        });
        // Clear on a double so a third click starts a fresh pair rather than
        // firing again on every click of a rapid run.
        self.last = (!double).then_some((x, y, now));
        double
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_contains_its_own_corners_and_nothing_past_them() {
        let r = Rect::new(4, 2, 3, 2);
        assert!(hit(r, 4, 2));
        assert!(hit(r, 6, 3));
        assert!(!hit(r, 7, 3), "one column past the right edge");
        assert!(!hit(r, 6, 4), "one row past the bottom edge");
        assert!(!hit(r, 3, 2));
        assert!(!hit(r, 4, 1));
    }

    #[test]
    fn an_empty_rect_is_never_hit() {
        assert!(!hit(Rect::new(0, 0, 0, 0), 0, 0));
    }

    #[test]
    fn two_quick_clicks_on_one_cell_are_a_double() {
        let t0 = Instant::now();
        let mut c = ClickTracker::new();
        assert!(!c.click_at(5, 5, t0));
        assert!(c.click_at(5, 5, t0 + Duration::from_millis(100)));
    }

    #[test]
    fn a_slow_second_click_is_a_new_first_one() {
        let t0 = Instant::now();
        let mut c = ClickTracker::new();
        assert!(!c.click_at(5, 5, t0));
        assert!(!c.click_at(5, 5, t0 + DOUBLE_CLICK));
        // And it is remembered, so the one after it can still pair.
        assert!(c.click_at(5, 5, t0 + DOUBLE_CLICK + Duration::from_millis(10)));
    }

    #[test]
    fn a_click_on_another_cell_does_not_pair() {
        let t0 = Instant::now();
        let mut c = ClickTracker::new();
        assert!(!c.click_at(5, 5, t0));
        assert!(!c.click_at(5, 6, t0 + Duration::from_millis(50)));
    }

    #[test]
    fn a_rapid_run_of_clicks_fires_on_every_second_one() {
        // Not on every click after the first: holding a key or resting on a
        // trackpad would otherwise activate a row repeatedly.
        let t0 = Instant::now();
        let mut c = ClickTracker::new();
        let fired: Vec<bool> = (0..5)
            .map(|i| c.click_at(1, 1, t0 + Duration::from_millis(50 * i)))
            .collect();
        assert_eq!(fired, vec![false, true, false, true, false]);
    }
}
