//! Decoration drawn over a panel's border.
//!
//! What lives here paints cells the `Block` widget has already drawn: the
//! corner gradient, and the constants that keep a panel's titles clear of it.
//! The actions a panel offers -- settings, close -- are words on a row of
//! their own now, in `header`, rather than a glyph on the border.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::theme::color::Rgb;
use crate::theme::Theme;

use super::header;
use super::rgb;

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
    cell.set_style(Style::default().fg(rgb(colour)).bg(rgb(t.panel_bg)));
}

/// Colour of the right-hand badge on the top border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Dim,
    Ok,
    Warn,
    Accent,
}

impl Tone {
    fn colour(self, t: &Theme) -> Rgb {
        match self {
            Tone::Dim => t.dim,
            Tone::Ok => t.ok,
            Tone::Warn => t.warn,
            Tone::Accent => t.accent,
        }
    }
}

/// Text at the right end of the top border, drawn as `" {text} \u{2550}"`
/// ([`TITLE_TRAIL`]): the mirror of a title's left end, for the odd fact a
/// panel wants to say about itself -- a count, a state, a mode -- that is not
/// its name.
#[derive(Debug, Clone, Copy)]
pub struct Badge<'a> {
    pub text: &'a str,
    pub tone: Tone,
}

/// Everything a panel needs to draw its own frame that is not its contents.
///
/// One struct per panel per frame, built fresh from whatever state the panel
/// is holding. Nothing here is kept between frames: recomputing a handful of
/// strings every draw is cheaper than a cache invalidated by every field that
/// could have changed underneath it.
pub struct Frame<'a, W: header::Word> {
    pub theme: &'a Theme,
    pub focused: bool,
    /// Left title. Uppercased here unless [`Frame::heading`] is set.
    pub title: &'a str,
    /// `" \u{2014} detail"` after the title, as typed -- a playlist's own
    /// name after `PLAYLIST`, a channel's topic after its title. Not
    /// uppercased, whatever the title is.
    pub detail: Option<&'a str>,
    /// `title` is the application's own letter-spaced name (`"S T A R /
    /// C O R D"`), drawn in `titlebar_active_fg` and bold rather than the
    /// ordinary header weight, and never uppercased -- there is no case to
    /// fold, the spacing already says what it is.
    pub heading: bool,
    pub badge: Option<Badge<'a>>,
    /// Bottom border, right-aligned, in `dim`, wrapped in one space each
    /// side: `" key verb \u{b7} key verb "`.
    pub footer: Option<&'a str>,
    /// Header-row words. Empty means no header row is reserved and none is
    /// drawn -- an overlay or a panel with nothing to offer there gets its
    /// whole inside back rather than a blank row it never uses.
    pub words: &'a [W],
}

/// A [`header::Word`] for a panel with no header row at all.
///
/// An empty enum rather than a unit one: nothing can be built to fill the
/// slice, so `&[]` is the only value a caller can pass, and `word` never has
/// to decide what an entry that does not exist says about itself.
#[derive(Debug, Clone, Copy)]
pub enum NoWords {}

impl header::Word for NoWords {
    fn word(self) -> Cow<'static, str> {
        match self {}
    }
}

/// The header-word slice for a panel that offers none, so a caller does not
/// have to spell `&[]` and pin down `NoWords` at every call site.
pub const NO_WORDS: &[NoWords] = &[];

/// The body [`frame`] will return, computed on its own so the mouse side can
/// ask for the same rect without drawing anything.
///
/// [`header::body`] when there are words to reserve a row for; the plain
/// inside of the border otherwise, since a header row nobody is drawing is a
/// row of content the panel never gets back.
pub fn body<W: header::Word>(area: Rect, words: &[W]) -> Rect {
    if words.is_empty() {
        Block::default().borders(Borders::ALL).inner(area)
    } else {
        header::body(area)
    }
}

/// Draw a panel's border, titles, corners and header row, and hand back what
/// is left for its contents.
///
/// Every panel starts with this and nothing else knows how one is framed, so
/// a change to the chrome is a change in one place. The body rect comes from
/// [`body`], which is also what the mouse tests against.
pub fn frame<W: header::Word>(area: Rect, buf: &mut Buffer, f: &Frame<'_, W>) -> Rect {
    let t = f.theme;
    let border = if f.focused {
        t.border_focused
    } else {
        t.border
    };

    // The title as every panel draws one: one border character in from the
    // corner, the name in capitals, a space, and the border again. The colour
    // is the header's, at the same weight focused or not -- focus is carried
    // by the border, and a title that changed weight with it said the same
    // thing twice.
    let text = if f.heading {
        f.title.to_string()
    } else {
        f.title.to_uppercase()
    };
    let full = match f.detail {
        Some(detail) => format!("{TITLE_LEAD}{text} \u{2014} {detail} "),
        None => format!("{TITLE_LEAD}{text} "),
    };
    let bare = format!("{TITLE_LEAD}{text} ");
    let style = if f.heading {
        Style::default()
            .fg(rgb(t.titlebar_active_fg))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(rgb(t.header_fg))
    };

    // Dropped a piece at a time rather than clipped mid-word. A title cut off
    // mid-way reads as a fault -- `PLAYLIST — Some Long Pla` is not the name
    // of anything -- so the detail goes first, and if the bare title still
    // does not fit, the whole title goes rather than any part of it.
    let room = area.width.saturating_sub(2);
    let full_width = crate::wrap::width_of(&full);
    let bare_width = crate::wrap::width_of(&bare);
    let (title_text, left_width) = if full_width <= room {
        (Some(full), full_width)
    } else if bare_width <= room {
        (Some(bare), bare_width)
    } else {
        (None, 0)
    };

    // The badge at the right end of the top border, the way the player keeps
    // ` bit-perfect ═` there. Dropped whole rather than clipped, same as the
    // title, and dropped first if there is not room for both.
    let badge = f.badge.and_then(|b| {
        let drawn = format!(" {}{TITLE_TRAIL}", b.text);
        let width = crate::wrap::width_of(&drawn);
        (left_width + 1 + width <= room).then_some((drawn, b.tone))
    });

    let footer = f
        .footer
        .map(|s| format!(" {s} "))
        .filter(|s| crate::wrap::width_of(s) <= room);

    // Double, as every panel is drawn. The title and badge share a treatment
    // with the border -- `TITLE_LEAD` and `TITLE_TRAIL` are `\u{2550}`, the
    // double horizontal -- so a single-line frame would put a heavier seam on
    // a lighter edge and the title would read as pasted on.
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(rgb(border)))
        .style(Style::default().bg(rgb(t.panel_bg)));

    if let Some(title_text) = &title_text {
        block = block.title(Span::styled(title_text.clone(), style));
    }
    if let Some((drawn, tone)) = &badge {
        block = block.title_top(
            Line::from(Span::styled(
                drawn.clone(),
                Style::default().fg(rgb(tone.colour(t))),
            ))
            .right_aligned(),
        );
    }
    if let Some(footer) = &footer {
        block = block.title_bottom(
            Line::from(Span::styled(
                footer.clone(),
                Style::default().fg(rgb(t.dim)),
            ))
            .right_aligned(),
        );
    }

    // The titles go on the block, and the block is drawn *before* the
    // corners: `render_corners` recolours every box-drawing cell in its run,
    // and the title's leading `═` is one. Drawn afterwards, the title would
    // keep its own colour on that cell and the gradient would stop dead at
    // the corner; drawn first, the gradient runs through it, and the corner
    // reads as one thing.
    block.render(area, buf);
    render_corners(area, buf, t, f.focused);

    if !f.words.is_empty() {
        header::render(area, f.words, buf, t);
    }

    body(area, f.words)
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

    /// A test word type for the tests in this module that need one but do not
    /// care what it says -- most of them draw no header row at all.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Word {
        Close,
    }

    impl header::Word for Word {
        fn word(self) -> Cow<'static, str> {
            "close".into()
        }
    }

    fn no_frills<'a>(theme: &'a Theme, title: &'a str) -> Frame<'a, NoWords> {
        Frame {
            theme,
            focused: false,
            title,
            detail: None,
            heading: false,
            badge: None,
            footer: None,
            words: NO_WORDS,
        }
    }

    fn top_row(buf: &Buffer, w: u16) -> String {
        (0..w).map(|x| buf[(x, 0)].symbol().to_string()).collect()
    }

    #[test]
    fn a_title_that_does_not_fit_drops_its_detail_first_then_itself() {
        let theme = test_theme("cosmic");
        let detail = "x".repeat(40);

        let area = Rect::new(0, 0, 30, 9);
        let mut buf = Buffer::empty(area);
        frame(
            area,
            &mut buf,
            &Frame {
                detail: Some(&detail),
                ..no_frills(&theme, "playlist")
            },
        );
        let top = top_row(&buf, 30);
        assert!(top.contains("\u{2550} PLAYLIST "), "the title fit: {top:?}");
        assert!(
            !top.contains('x'),
            "the detail should have been dropped: {top:?}"
        );

        let area = Rect::new(0, 0, 8, 9);
        let mut buf = Buffer::empty(area);
        frame(
            area,
            &mut buf,
            &Frame {
                detail: Some(&detail),
                ..no_frills(&theme, "playlist")
            },
        );
        let top = top_row(&buf, 8);
        assert!(
            !top.chars().any(|c| c.is_ascii_alphabetic()),
            "the title should have been dropped whole: {top:?}"
        );
    }

    #[test]
    fn a_title_is_uppercased_and_a_heading_is_not() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 9);

        let mut buf = Buffer::empty(area);
        frame(area, &mut buf, &no_frills(&theme, "playlist"));
        assert!(top_row(&buf, 40).contains("PLAYLIST"));

        let mut buf = Buffer::empty(area);
        frame(
            area,
            &mut buf,
            &Frame {
                heading: true,
                ..no_frills(&theme, "s t a r / c o r d")
            },
        );
        let top = top_row(&buf, 40);
        assert!(top.contains("s t a r / c o r d"), "{top:?}");
        assert!(
            !top.contains("S T A R"),
            "a heading should not be uppercased: {top:?}"
        );
    }

    #[test]
    fn no_words_reserves_no_header_row() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 9);

        let plain = Block::default().borders(Borders::ALL).inner(area);
        assert_eq!(body(area, NO_WORDS), plain);
        assert_eq!(body(area, &[Word::Close]), header::body(area));

        let mut buf = Buffer::empty(area);
        let got = frame(area, &mut buf, &no_frills(&theme, "playlist"));
        assert_eq!(got, plain);

        let mut buf = Buffer::empty(area);
        let got = frame(
            area,
            &mut buf,
            &Frame {
                theme: &theme,
                focused: false,
                title: "playlist",
                detail: None,
                heading: false,
                badge: None,
                footer: None,
                words: &[Word::Close],
            },
        );
        assert_eq!(got, header::body(area));
    }

    #[test]
    fn the_badge_ends_clear_of_the_corner() {
        // A border character between the badge and the corner, matching the
        // one between the corner and the title at the other end.
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 9);
        let mut buf = Buffer::empty(area);
        frame(
            area,
            &mut buf,
            &Frame {
                badge: Some(Badge {
                    text: "3/40",
                    tone: Tone::Dim,
                }),
                ..no_frills(&theme, "playlist")
            },
        );
        let top = top_row(&buf, 40);
        assert!(top.contains("3/40"), "the badge is missing: {top:?}");
        assert!(
            top.trim_end().ends_with("\u{2550}\u{2557}"),
            "no buffer before the right corner: {top:?}"
        );
    }

    /// Asserted on the glyphs rather than on `BorderType`, because what a
    /// reader sees is the character in the cell.
    #[test]
    fn the_frame_is_drawn_in_double_lines() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 12, 5);
        let mut buf = Buffer::empty(area);
        frame(area, &mut buf, &no_frills(&theme, ""));
        let at = |x: u16, y: u16| buf[(x, y)].symbol().to_string();
        assert_eq!(
            [at(0, 0), at(11, 0), at(11, 4), at(0, 4)],
            ["\u{2554}", "\u{2557}", "\u{255d}", "\u{255a}"],
            "the corners are not the double-line ones"
        );
        assert_eq!(at(6, 0), "\u{2550}", "the top edge is not double");
        assert_eq!(at(6, 4), "\u{2550}", "the bottom edge is not double");
        assert_eq!(at(0, 2), "\u{2551}", "the left edge is not double");
        assert_eq!(at(11, 2), "\u{2551}", "the right edge is not double");
    }

    #[test]
    fn the_panel_is_filled_with_panel_bg() {
        // `tint` used to force every recoloured border cell back to the
        // global background, which put a seam around the corners of any
        // theme whose panel sits on its own background. Checked on a corner
        // cell, since that is the one `tint` touches -- the rest of the
        // panel gets `panel_bg` from the block's own style regardless.
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 9);
        let mut buf = Buffer::empty(area);
        frame(area, &mut buf, &no_frills(&theme, "playlist"));
        assert_eq!(buf[(0, 0)].style().bg, Some(rgb(theme.panel_bg)));
    }

    #[test]
    fn the_footer_sits_on_the_bottom_border_right_aligned() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 9);
        let mut buf = Buffer::empty(area);
        frame(
            area,
            &mut buf,
            &Frame {
                footer: Some("enter play \u{b7} esc close"),
                ..no_frills(&theme, "playlist")
            },
        );
        let bottom: String = (0..40)
            .map(|x| buf[(x, area.height - 1)].symbol().to_string())
            .collect();
        assert!(bottom.contains("enter play"), "{bottom:?}");
        assert!(
            bottom.trim_end().ends_with("close \u{255d}") || bottom.ends_with("close \u{255d}"),
            "the footer is not flush against the right border: {bottom:?}"
        );
    }
}
