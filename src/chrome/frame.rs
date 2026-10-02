//! A panel's frame: the border, its title, and what a panel hands this
//! module to draw both.
//!
//! A frame is one border colour all the way round -- `border` or
//! `border_focused`, depending on focus, and nothing else. What lives here is
//! [`frame`] itself, the constants that keep a panel's titles clear of the
//! corners they sit next to, and [`Frame`], the struct a panel builds fresh
//! each draw to say what its border, title, badge and footer should say. The
//! actions a panel offers -- settings, close -- are words on a row of their
//! own now, in `header`, rather than a glyph on the border.

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

thread_local! {
    static PADDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Scoped pixel chrome geometry. Restores the previous presentation on drop,
/// so a controller cannot change another app or the ordinary terminal layout.
pub struct PaddingScope {
    previous: bool,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for PaddingScope {
    fn drop(&mut self) {
        PADDED.with(|value| value.set(self.previous));
    }
}
/// Keep drawing and hit testing inside the same presentation scope.
pub fn padding_scope(enabled: bool) -> PaddingScope {
    PaddingScope {
        previous: PADDED.with(|value| value.replace(enabled)),
        _thread: std::marker::PhantomData,
    }
}
pub(super) fn padded() -> bool {
    PADDED.with(std::cell::Cell::get)
}
/// Extra height reserved for an inset title and content gap in pixel mode.
pub fn extra_rows() -> u16 {
    if padded() {
        2
    } else {
        0
    }
}

/// What a panel's title starts with.
///
/// One border character between the corner and the text, so the title reads
/// as sitting on the frame rather than floating clear of it.
pub const TITLE_LEAD: &str = "\u{2550} ";

/// What a right-aligned heading ends with: the mirror of [`TITLE_LEAD`].
///
/// One border character between the last of the text and the corner, so both
/// ends of the top border read the same way round.
pub const TITLE_TRAIL: &str = " \u{2550}";

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
        let mut inner = Block::default().borders(Borders::ALL).inner(area);
        if padded() {
            inner.x = inner.x.saturating_add(1).min(area.right());
            inner.width = inner.width.saturating_sub(2);
            inner.y = inner.y.saturating_add(2).min(area.bottom());
            inner.height = inner.height.saturating_sub(2);
        }
        inner
    } else {
        header::body(area)
    }
}

/// Draw a panel's border, titles and header row, and hand back what is left
/// for its contents.
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
    let room = area.width.saturating_sub(if padded() { 4 } else { 2 });
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
    let border_style = Style::default().fg(rgb(border));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(border_style)
        .style(Style::default().bg(rgb(t.panel_bg)));

    // `TITLE_LEAD` and `TITLE_TRAIL`'s `\u{2550}` is border, not text, so it
    // keeps the border's own colour rather than the title's or badge's --
    // a frame is one colour all the way round, and the only cells that say
    // otherwise are the words themselves.
    if let Some(title_text) = &title_text.as_ref().filter(|_| !padded()) {
        let rest = title_text.strip_prefix(TITLE_LEAD).unwrap_or(title_text);
        block = block.title(Line::from(vec![
            Span::styled(TITLE_LEAD, border_style),
            Span::styled(rest.to_string(), style),
        ]));
    }
    if let Some((drawn, tone)) = &badge.as_ref().filter(|_| !padded()) {
        let glyph = &TITLE_TRAIL[1..];
        let main = drawn.strip_suffix(glyph).unwrap_or(drawn);
        block = block.title_top(
            Line::from(vec![
                Span::styled(main.to_string(), Style::default().fg(rgb(tone.colour(t)))),
                Span::styled(glyph, border_style),
            ])
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

    block.render(area, buf);
    if padded() && area.height > 2 && area.width > 4 {
        let y = area.y + 1;
        if let Some(title) = &title_text {
            let text = title.strip_prefix(TITLE_LEAD).unwrap_or(title).trim_end();
            buf.set_stringn(area.x + 2, y, text, usize::from(room), style);
        }
        if let Some((drawn, tone)) = &badge {
            let text = drawn.trim().trim_end_matches('═').trim();
            let width = crate::wrap::width_of(text);
            buf.set_string(
                area.right() - 2 - width,
                y,
                text,
                Style::default().fg(rgb(tone.colour(t))),
            );
        }
    }

    if !f.words.is_empty() {
        header::render(area, f.words, buf, t);
    }

    body(area, f.words)
}

#[cfg(test)]
mod tests {
    use super::super::test_theme;
    use super::*;

    #[test]
    fn inset_titles_actions_and_body_share_geometry_and_restore_scope() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Action {
            Close,
        }
        impl header::Word for Action {
            fn word(self) -> Cow<'static, str> {
                "close".into()
            }
        }
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        assert_eq!(extra_rows(), 0);
        {
            let _chrome = padding_scope(true);
            let content = frame(
                area,
                &mut buf,
                &Frame {
                    theme: &theme,
                    focused: true,
                    title: "preview",
                    detail: None,
                    heading: false,
                    badge: None,
                    footer: None,
                    words: &[Action::Close],
                },
            );
            assert_eq!(buf[(2, 0)].symbol(), "═");
            assert_eq!(buf[(2, 1)].symbol(), "P");
            assert_eq!((content.x, content.y), (2, 4));
            let slots = header::slots(area, &[Action::Close]);
            let (_, hit) = slots[0];
            assert_eq!(hit.y, 2);
            assert_eq!(
                header::hit(area, &[Action::Close], hit.x, hit.y),
                Some(Action::Close)
            );
            assert_eq!(header::hit(area, &[Action::Close], hit.x, 1), None);
            {
                let _compact = padding_scope(false);
                assert_eq!(body(area, &[Action::Close]).y, 2);
            }
            assert_eq!(body(area, &[Action::Close]).y, 4);
        }
        assert_eq!(extra_rows(), 0);
        assert_eq!(body(area, &[Action::Close]).y, 2);
    }

    /// Every border cell -- both corners and the straight runs between them
    /// -- is the one colour a frame draws in, focused or not.
    ///
    /// Skips the title and badge text on the top border and the footer text
    /// on the bottom one: those cells hold letters, not box-drawing
    /// characters, so filtering on the glyph is what tells a border cell from
    /// a text cell without having to know where the words landed.
    #[test]
    fn a_focused_frame_is_one_colour_all_the_way_round() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 40, 10);

        for (focused, want) in [(true, theme.border_focused), (false, theme.border)] {
            let mut buf = Buffer::empty(area);
            frame(
                area,
                &mut buf,
                &Frame {
                    theme: &theme,
                    focused,
                    title: "playlist",
                    detail: None,
                    heading: false,
                    badge: Some(Badge {
                        text: "3/40",
                        tone: Tone::Dim,
                    }),
                    footer: None,
                    words: NO_WORDS,
                },
            );
            for y in 0..area.height {
                for x in 0..area.width {
                    let cell = &buf[(x, y)];
                    let is_border = cell
                        .symbol()
                        .chars()
                        .next()
                        .is_some_and(|c| ('\u{2500}'..='\u{257f}').contains(&c));
                    if !is_border {
                        continue;
                    }
                    let fg = match cell.style().fg {
                        Some(ratatui::style::Color::Rgb(r, g, b)) => Rgb::new(r, g, b),
                        other => panic!("cell {x},{y} is not an rgb colour: {other:?}"),
                    };
                    assert_eq!(
                        fg, want,
                        "focused={focused}: cell {x},{y} is not the frame colour"
                    );
                }
            }
        }
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
