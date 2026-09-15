//! The box a panel opens on top of everything else: settings, help, a jump
//! target. One shape, so a terminal too short for one of them is a terminal
//! too short for all of them in the same way, and a reader who has learned
//! where one overlay sits has learned where they all sit.
//!
//! No background dimming. The overlay is drawn last and its own border and
//! fill are already enough to read as on top; darkening everything behind it
//! is a cost paid on every frame for a cue the ordering already gives for
//! free.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme::Theme;

use super::frame::{self, Frame, NO_WORDS};

/// Where the box sits vertically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The middle of the area: settings, the help overlay -- anything read
    /// with both hands still on the keyboard, no typing involved.
    Centre,
    /// A third of the way down, the position starcord's typing boxes use so
    /// the eye and the text land high rather than in the middle of whatever
    /// is behind them.
    Upper,
}

/// Columns kept clear at the sides, so a wide overlay never touches the
/// terminal's own edge.
pub const MARGIN_X: u16 = 4;

/// Rows kept clear top and bottom, same reasoning as [`MARGIN_X`].
pub const MARGIN_Y: u16 = 2;

/// Where an overlay's box goes: centred (or upper-anchored) in `area`, sized
/// between `width.0` and `width.1` columns and up to `want_h` rows, never
/// smaller than `min_h` and never larger than `area` itself.
///
/// `width` and `min_h` are the two ends of the same trade: a box wants to be
/// big enough to read comfortably and small enough to leave the panel behind
/// it visible at the edges. Every caller answers that trade differently --
/// the help overlay wants to be wide, a settings list wants to be exactly as
/// tall as its rows -- so both ends are parameters rather than a constant
/// tuned for one of them.
///
/// Guarded at every step so a terminal smaller than the box's own minimums
/// gets the largest box that still fits rather than a panic: `min_w` and
/// `max_w` are sorted before the clamp, which is the one call here that
/// panics if its bounds arrive the wrong way round, and both dimensions are
/// capped to `area` last, after every other adjustment.
pub fn rect(area: Rect, width: (u16, u16), want_h: u16, min_h: u16, anchor: Anchor) -> Rect {
    let (min_w, max_w) = width;
    let (lo, hi) = if min_w <= max_w {
        (min_w, max_w)
    } else {
        (max_w, min_w)
    };
    let w = area
        .width
        .saturating_sub(MARGIN_X)
        .clamp(lo, hi)
        .min(area.width);
    let h = want_h
        .min(area.height.saturating_sub(MARGIN_Y))
        .max(min_h)
        .min(area.height);

    let y = match anchor {
        Anchor::Centre => area.y + (area.height.saturating_sub(h)) / 2,
        Anchor::Upper => area.y + (area.height.saturating_sub(h)) / 3,
    };
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y,
        width: w,
        height: h,
    }
}

/// Where an overlay's contents go, inside the border [`render`] draws.
///
/// A caller that needs this before it has anything to draw -- the mouse side,
/// testing a click against rows it has not rendered -- asks here rather than
/// rebuilding the border's inset by hand.
pub fn inner(r: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(r)
}

/// Everything [`render`] needs to frame a box that is not its contents.
pub struct Overlay<'a> {
    pub theme: &'a Theme,
    /// Uppercased by [`frame::frame`], same as any other panel's title.
    pub title: &'a str,
    /// `" \u{2014} detail"` after the title, as typed.
    pub detail: Option<&'a str>,
    /// Bottom border, right-aligned: `" key verb \u{b7} key verb "`.
    pub footer: Option<&'a str>,
}

/// Clear `r`, then frame it as the one thing focused on screen.
///
/// An overlay is always focused -- it is the thing on top, and there is
/// nothing behind it a reader could mean instead -- so this does not take a
/// `focused` flag the way [`Frame`] does for a docked panel. It never reserves
/// a header row either: an overlay's actions are its footer, not a row of
/// words a click could confuse with the panel underneath.
///
/// Returns the same rect [`inner`] would give for `r`, so a caller that wants
/// it before drawing anything -- a mouse hit test -- and a caller drawing
/// content right after this call agree on where that content goes.
pub fn render(r: Rect, buf: &mut Buffer, o: &Overlay<'_>) -> Rect {
    Clear.render(r, buf);
    frame::frame(
        r,
        buf,
        &Frame {
            theme: o.theme,
            focused: true,
            title: o.title,
            detail: o.detail,
            heading: false,
            badge: None,
            footer: o.footer,
            words: NO_WORDS,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_theme;
    use super::*;
    use crate::chrome::rgb;

    #[test]
    fn the_box_stays_inside_the_terminal() {
        for (w, h) in [(24u16, 8u16), (40, 10), (80, 24), (100, 30), (200, 60)] {
            for anchor in [Anchor::Centre, Anchor::Upper] {
                let area = Rect::new(0, 0, w, h);
                let r = rect(area, (24, 52), 20, 6, anchor);
                assert!(
                    r.x >= area.x && r.x + r.width <= area.x + area.width,
                    "{w}x{h} {anchor:?} overflows across"
                );
                assert!(
                    r.y >= area.y && r.y + r.height <= area.y + area.height,
                    "{w}x{h} {anchor:?} overflows down"
                );
            }
        }

        // A handful of (min, max, want, min_h) combinations, including a
        // minimum wider than the terminal itself and bounds given the wrong
        // way round -- neither should panic, and the box still has to fit.
        let area = Rect::new(0, 0, 30, 12);
        for (min_w, max_w, want_h, min_h) in [
            (24u16, 52u16, 20u16, 6u16),
            (52, 24, 20, 6),
            (200, 300, 3, 1),
            (0, 0, 0, 0),
            (10, 10, 100, 100),
        ] {
            for anchor in [Anchor::Centre, Anchor::Upper] {
                let r = rect(area, (min_w, max_w), want_h, min_h, anchor);
                assert!(
                    r.x + r.width <= area.width,
                    "{min_w},{max_w},{want_h},{min_h}: overflows across"
                );
                assert!(
                    r.y + r.height <= area.height,
                    "{min_w},{max_w},{want_h},{min_h}: overflows down"
                );
            }
        }
    }

    #[test]
    fn upper_anchor_sits_higher_than_centre() {
        let area = Rect::new(0, 0, 100, 30);
        let centre = rect(area, (24, 52), 12, 6, Anchor::Centre);
        let upper = rect(area, (24, 52), 12, 6, Anchor::Upper);
        assert!(
            upper.y < centre.y,
            "upper did not sit higher: {upper:?} vs {centre:?}"
        );
    }

    #[test]
    fn render_returns_the_inner_rect_and_draws_the_uppercase_title() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);

        let r = rect(area, (24, 52), 10, 6, Anchor::Centre);
        let inside = inner(r);
        // A marker where the inner rect will land, so "cleared" means
        // something: the marker has to be gone afterwards.
        buf.set_string(
            inside.x,
            inside.y,
            "xxxxxxxxxx",
            ratatui::style::Style::default(),
        );

        let got = render(
            r,
            &mut buf,
            &Overlay {
                theme: &theme,
                title: "jump to",
                detail: None,
                footer: Some("esc close"),
            },
        );

        assert_eq!(got, inner(r));

        let top: String = (r.x..r.x + r.width)
            .map(|x| buf[(x, r.y)].symbol().to_string())
            .collect();
        assert!(top.contains("\u{2550} JUMP TO "), "{top:?}");

        let bottom: String = (r.x..r.x + r.width)
            .map(|x| buf[(x, r.y + r.height - 1)].symbol().to_string())
            .collect();
        assert!(
            bottom.trim_end().ends_with("esc close \u{255d}")
                || bottom.ends_with("esc close \u{255d}"),
            "the footer is not flush against the right border: {bottom:?}"
        );

        assert_ne!(
            buf[(inside.x, inside.y)].symbol(),
            "x",
            "the marker was not cleared"
        );
        assert_eq!(
            buf[(inside.x, inside.y)].style().bg,
            Some(rgb(theme.panel_bg))
        );
    }
}
