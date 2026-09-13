//! Decoration drawn over a panel's border.
//!
//! What lives here paints cells the `Block` widget has already drawn: the
//! corner gradient, and the constants that keep a panel's titles clear of it.
//! The actions a panel offers -- settings, close -- are words on a row of
//! their own now, in `header`, rather than a glyph on the border.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::theme::color::Rgb;
use crate::theme::Theme;

fn rgb(c: Rgb) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(c.r, c.g, c.b)
}

/// How far each corner sits from the border toward the foreground.
///
/// Measured from the *border*, not from the background. A theme sets its own
/// border weight, so a fixed distance from the background put Latte's faintest
/// corner on the wrong side of its border and made it less visible than the
/// line it decorates. From the border, every corner is between the border and
/// the foreground by construction, whichever way round the theme runs.
///
/// This is a gradient in weight, not in hue -- grey in a neutral theme and
/// faintly tinted in a tinted one, which is what "grey" means for whichever
/// theme is loaded.
///
/// Clockwise from the top left, dimming as it goes, so the four read as one
/// gradient turning around the panel rather than four unrelated marks.
///
/// Brightest at the top left because that is the corner the eye starts on.
/// Running the other way put the faintest corner there, and a panel's most
/// looked-at corner was the one hardest to see.
///
/// All four sit above the border's own weight -- a corner dimmer than the
/// border it decorates is not a corner, and the run out of it has nothing to
/// fade from -- but only just. Against Cosmic's `3b3b3b` border these land
/// between `545454` and `434343`, where the brightest used to be `717171`:
/// more than twice the contrast, and it read as four bright marks stuck to a
/// dim frame rather than as a frame that happens to catch the light. The
/// gradient should be something you notice on the second look.
const CORNER_WEIGHTS: [f64; 4] = [0.20, 0.15, 0.10, 0.06];

/// What a panel's title starts with.
///
/// One border character between the corner and the text, so the title reads
/// as sitting on the frame rather than floating clear of it. The run out of
/// the top-left corner travels over this one cell and is covered by the title
/// beyond it -- the trade taken deliberately, since the title is what the
/// panel is called and the run is decoration.
pub const TITLE_LEAD: &str = "\u{2550} ";

/// What a right-aligned heading ends with: the mirror of [`TITLE_LEAD`].
///
/// One border character between the last of the text and the corner, so both
/// ends of the top border read the same way round.
pub const TITLE_TRAIL: &str = " \u{2550}";

/// Cells either side of a corner that carry the fade back to the border.
///
/// Twice as many across as down. A terminal cell is about twice as tall as it
/// is wide, so the same count both ways draws a horizontal run half the length
/// of the vertical one and the corner comes out lopsided.
///
/// Short on purpose. A long run ramps out of one corner and back into the
/// next, which leaves half of every edge reading dark to bright; kept to a few
/// cells it reads as a corner rather than as a ramp along the whole edge.
///
/// Panel titles sit at the left edge and cover the top-left run. That is the
/// trade taken deliberately: the run is decoration and the title is what the
/// panel is called, so the title gets the cells. The other three corners, and
/// both vertical runs, are unaffected.
const CORNER_RUN_ACROSS: u16 = 4;
const CORNER_RUN_DOWN: u16 = 2;

/// The four corners, each a different weight of the frame's grey, fading back
/// into the border along both edges that meet there.
pub fn render_corners(area: Rect, buf: &mut Buffer, t: &Theme, focused: bool) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    // Focus is carried by the border's colour, which this decoration inherits
    // rather than adds to. `border_focused` is already tinted toward the
    // accent, so building the corners from it gives them the hue for free and
    // leaves this function doing the one job it had: a weight gradient that
    // turns around the panel.
    //
    // The alternative -- a grey border with only the corners tinted -- was
    // tried and is too quiet to find. The cost of a coloured border is that
    // two docked panels share an edge, so a focused panel's bottom line is
    // also the top of whatever is below it.
    // The border this decoration sits on. A focused panel draws a tinted
    // frame, so the corners have to be built from *that* colour and fade back
    // into it -- built from the unfocused grey they would end on a different
    // colour from the line they decorate, and every run would finish with a
    // visible step.
    let base = if focused { t.border_focused } else { t.border };
    let (x0, y0) = (area.x, area.y);
    let (x1, y1) = (area.x + area.width - 1, area.y + area.height - 1);

    // Each corner, and which way its two runs travel from it.
    let corners = [
        ((x0, y0), (1i32, 1i32)),
        ((x1, y0), (-1, 1)),
        ((x1, y1), (-1, -1)),
        ((x0, y1), (1, -1)),
    ];

    for (((cx, cy), (dx, dy)), weight) in corners.into_iter().zip(CORNER_WEIGHTS) {
        // The unfocused colour, then tinted toward the accent -- not mixed
        // toward the accent instead of the foreground. Replacing the target
        // changes the lightness as well as the hue, and in a theme whose
        // accent is darker than its foreground (nord) that made the focused
        // corner *dimmer* than the unfocused one. Tinting the result moves
        // hue while leaving the gradient's weight alone.
        let lit = base.mix(t.fg, weight);
        tint(buf, area, cx, cy, lit, t);

        // Out along both edges, ending on the border's own grey so the run
        // closes without a seam.
        for k in 1..=CORNER_RUN_ACROSS {
            let mix = k as f64 / (CORNER_RUN_ACROSS + 1) as f64;
            let x = (cx as i32 + dx * k as i32).max(0) as u16;
            tint(buf, area, x, cy, lit.mix(base, mix), t);
        }
        for k in 1..=CORNER_RUN_DOWN {
            let mix = k as f64 / (CORNER_RUN_DOWN + 1) as f64;
            let y = (cy as i32 + dy * k as i32).max(0) as u16;
            tint(buf, area, cx, y, lit.mix(base, mix), t);
        }
    }
}

/// Recolour one border cell, if that is what it is.
///
/// Only cells already holding a box-drawing character are touched. The top
/// border also carries a panel's title, and a run long
/// enough to be worth drawing reaches them; recolouring those would put the
/// frame's grey on text that is meant to be read.
fn tint(buf: &mut Buffer, area: Rect, x: u16, y: u16, colour: Rgb, t: &Theme) {
    if x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height {
        return;
    }
    let cell = &mut buf[(x, y)];
    let is_border = cell
        .symbol()
        .chars()
        .next()
        .is_some_and(|c| ('\u{2500}'..='\u{257f}').contains(&c));
    if !is_border {
        return;
    }
    cell.set_style(Style::default().fg(rgb(colour)).bg(rgb(t.bg)));
}

#[cfg(test)]
mod tests {
    use super::super::test_theme;
    use super::*;

    /// A bordered panel with the corner treatment drawn over it, as the real
    /// panels do it. The decoration only touches cells that already hold a
    /// border, so an empty buffer would come back untouched.
    fn framed(theme: &Theme, w: u16, h: u16) -> Buffer {
        framed_focus(theme, w, h, false)
    }

    fn framed_focus(theme: &Theme, w: u16, h: u16, focused: bool) -> Buffer {
        use ratatui::widgets::{Block, Borders, Widget};
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        // Styled like the real panels: untinted cells keep the border's own
        // colour, which is what the run has to fade into.
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(rgb(theme.border)))
            .render(area, &mut buf);
        render_corners(area, &mut buf, theme, focused);
        buf
    }

    fn fg_at(buf: &Buffer, x: u16, y: u16) -> Rgb {
        match buf[(x, y)].style().fg {
            Some(ratatui::style::Color::Rgb(r, g, b)) => Rgb::new(r, g, b),
            other => panic!("cell {x},{y} is not an rgb colour: {other:?}"),
        }
    }

    /// Focus lands on the corners, and only on the corners.
    ///
    /// Not on the border: two docked panels share an edge, so lighting one
    /// panel's frame lights half of its neighbour's.
    /// Perceptual distance, not a contrast ratio.
    ///
    /// Contrast is a luminance ratio, so two colours of similar lightness and
    /// wildly different hue read as identical to it -- which is exactly the
    /// case for a corner and an accent in several themes.
    fn oklab_distance(a: Rgb, b: Rgb) -> f64 {
        let (a, b) = (a.to_oklab(), b.to_oklab());
        ((a.l - b.l).powi(2) + (a.a - b.a).powi(2) + (a.b - b.b).powi(2)).sqrt()
    }

    #[test]
    fn a_focused_panel_lifts_its_corners_toward_the_accent() {
        for name in ["cosmic", "catppuccin-mocha", "nord", "gruvbox-dark"] {
            let theme = test_theme(name);
            let dim = fg_at(&framed_focus(&theme, 40, 10, false), 0, 0);
            let lit = fg_at(&framed_focus(&theme, 40, 10, true), 0, 0);
            assert_ne!(dim, lit, "{name}: focus does not change the corner");
            assert!(
                oklab_distance(lit, theme.accent) < oklab_distance(dim, theme.accent),
                "{name}: the focused corner is not nearer the accent"
            );
        }
    }

    /// The gradient survives the tint.
    ///
    /// The corners' job is a weight gradient turning around the panel, and it
    /// has to still be legible once they are built from a coloured border
    /// rather than a grey one -- a focused panel whose four corners came out
    /// the same shade would have lost the decoration to the focus mark.
    #[test]
    fn a_focused_panel_keeps_its_corner_gradient() {
        for name in ["cosmic", "catppuccin-mocha", "nord", "gruvbox-dark"] {
            let theme = test_theme(name);
            let buf = framed_focus(&theme, 40, 10, true);
            let corners = [(0, 0), (39, 0), (39, 9), (0, 9)].map(|(x, y)| fg_at(&buf, x, y));
            assert!(
                oklab_distance(corners[0], corners[2]) > 0.02,
                "{name}: the brightest and dimmest corners are the same shade"
            );
        }
    }

    fn corner_colours(theme: &Theme) -> [Rgb; 4] {
        let buf = framed(theme, 40, 10);
        [(0, 0), (39, 0), (39, 9), (0, 9)].map(|(x, y)| fg_at(&buf, x, y))
    }

    #[test]
    fn every_corner_is_a_different_weight() {
        let theme = test_theme("cosmic");
        let corners = corner_colours(&theme);
        let mut seen = corners.to_vec();
        seen.dedup();
        assert_eq!(seen.len(), 4, "two corners share a colour: {corners:?}");

        // And they change in one direction, so the four read as one gradient
        // turning around the panel rather than four unrelated marks.
        let lift = |c: Rgb| c.r as u32 + c.g as u32 + c.b as u32;
        for pair in corners.windows(2) {
            assert!(
                lift(pair[1]) < lift(pair[0]),
                "the gradient does not run: {corners:?}"
            );
        }
        // Brightest where the eye starts.
        assert_eq!(
            corners.iter().max_by_key(|c| lift(**c)).map(|c| lift(*c)),
            Some(lift(corners[0])),
            "the top left is not the brightest corner"
        );
    }

    #[test]
    fn the_corners_are_grey_rather_than_the_spectrum() {
        // They used to sample the visualizer's ramp, which put four hues on a
        // frame that is meant to be quiet.
        let theme = test_theme("cosmic");
        for c in corner_colours(&theme) {
            let (lo, hi) = (c.r.min(c.g).min(c.b) as i32, c.r.max(c.g).max(c.b) as i32);
            assert!(hi - lo <= 24, "corner {c:?} is too saturated to be a grey");
        }
    }

    #[test]
    fn corners_stand_out_from_the_background_in_either_polarity() {
        // The gradient runs along the theme's own background-to-foreground
        // axis, so in a light theme it runs *down* into dark text rather than
        // up into light. Absolute lightness is therefore the wrong question --
        // what has to hold is that a corner is visible against its own panel,
        // and more visible than the border it decorates.
        for name in ["cosmic", "catppuccin-latte", "nord", "gruvbox-dark"] {
            let theme = test_theme(name);
            for c in corner_colours(&theme) {
                let against_bg = theme.bg.contrast(c);
                assert!(
                    against_bg > theme.bg.contrast(theme.border),
                    "{name}: corner {c:?} is no more visible than the border"
                );
            }
        }
    }

    #[test]
    fn the_run_is_longer_across_than_down() {
        // A cell is about twice as tall as it is wide, so matching counts
        // would draw a lopsided corner.
        assert_eq!(CORNER_RUN_ACROSS, CORNER_RUN_DOWN * 2);
    }

    #[test]
    fn the_run_fades_and_the_edge_beyond_it_is_flat() {
        let theme = test_theme("cosmic");
        let buf = framed(&theme, 40, 10);
        let lift = |c: Rgb| c.r as i32 + c.g as i32 + c.b as i32;

        for run in [
            (0..=CORNER_RUN_ACROSS)
                .map(|k| fg_at(&buf, k, 0))
                .collect::<Vec<_>>(),
            (0..=CORNER_RUN_DOWN)
                .map(|k| fg_at(&buf, 0, k))
                .collect::<Vec<_>>(),
        ] {
            for pair in run.windows(2) {
                assert!(
                    lift(pair[1]) <= lift(pair[0]),
                    "the run does not fade: {run:?}"
                );
            }
            assert!(
                lift(run[0]) > lift(*run.last().unwrap()),
                "the run is flat: {run:?}"
            );
        }

        // The middle of an edge is left alone, so it does not read as one
        // ramp from corner to corner.
        assert_eq!(fg_at(&buf, 20, 0), theme.border);
        assert_eq!(fg_at(&buf, 0, 5), theme.border);
    }

    #[test]
    fn the_decoration_leaves_a_panel_title_alone() {
        // The run reaches into the top border, which is where a title sits.
        // Recolouring it would put the frame's grey on text meant to be read.
        use ratatui::widgets::{Block, Borders, Widget};
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        Block::default()
            .borders(Borders::ALL)
            .title("PLAYLIST")
            .render(area, &mut buf);
        let before: Vec<String> = (1..9).map(|x| format!("{:?}", buf[(x, 0)])).collect();
        render_corners(area, &mut buf, &theme, false);
        let after: Vec<String> = (1..9).map(|x| format!("{:?}", buf[(x, 0)])).collect();
        assert_eq!(before, after, "the title was recoloured");
    }

    #[test]
    fn a_panel_too_small_to_have_corners_is_left_alone() {
        let theme = test_theme("cosmic");
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        render_corners(Rect::new(0, 0, 1, 1), &mut buf, &theme, false);
        render_corners(Rect::new(0, 0, 0, 0), &mut buf, &theme, false);
    }
}
