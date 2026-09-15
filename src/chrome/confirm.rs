//! "Are you sure?" -- the one shape both applications' confirmation dialogues
//! agree on once their disagreements are resolved.
//!
//! STAR/FOLD's dialogue sizes itself to its body and answers with the
//! question's own verbs -- "delete" and "keep", not a fixed "yes" and "no" --
//! because a reader deciding between the two cannot press the wrong one out
//! of habit the way they can with `y`/`n` alone: muscle memory answers `y` to
//! every prompt it has ever seen, and the word under the cursor is what makes
//! this one different. STAR/CORD's dialogue puts those verbs on the bottom
//! border rather than a line of the body, which is where the rest of this
//! crate's chrome already keeps its actions and where this one keeps them
//! too.
//!
//! What neither app had: `Enter` is not bound to `Yes`. STAR/FOLD asks this
//! before a permanent delete, and a dialogue whose default key is the one a
//! reader's thumb is already resting on -- because it just finished naming a
//! file, or dismissing the panel behind this one -- is a dialogue that
//! deletes something on a keystroke meant for whatever came before it.
//! Saying yes costs a deliberate `y`; saying no is cheap, on `n`, `N` or
//! `Esc`, since backing out should never be the harder answer.
//!
//! [`layout`] is the one computation both [`render`] and a caller's own mouse
//! handling read, so the box a reader sees and the box a click is tested
//! against can never drift apart -- same doctrine as the rest of this crate's
//! chrome.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::theme::Theme;
use crate::wrap;

use super::overlay::{self, Anchor};
use super::rgb;

/// The question. Apps wrap this in their own struct carrying whatever
/// `Pending` payload the answer applies to -- what that payload is is not
/// this crate's business, only the words asking about it are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    /// Uppercased on the top border, same as any other panel's title:
    /// "delete", "quit", "clear queue".
    pub title: String,
    /// Paragraphs, each wrapped to the box's own width by [`layout`]. An
    /// empty string is a blank line between two paragraphs, not a row a
    /// caller has to leave out.
    pub body: Vec<String>,
    /// The verb on the answer that goes ahead with it: "delete", "quit",
    /// "yes" for a question with nothing more specific to say.
    pub yes: &'static str,
    /// The verb on the answer that backs out: "keep", "stay", "no".
    pub no: &'static str,
}

/// Where the box lands, the body already wrapped to fit it, and where the two
/// answers sit on the bottom border -- everything [`render`] draws and a
/// click is tested against, worked out once so the two cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub rect: Rect,
    pub inner: Rect,
    /// Wrapped rows that fit inside `inner`, in order. A body longer than the
    /// box is truncated rather than pushed off the bottom of the terminal --
    /// the alternative to a scrollbar on a yes/no dialogue is not scrolling
    /// it, it is not writing a body that long.
    pub body_rows: Vec<String>,
    /// Column span `[start, end)` on `footer_y` of `"y {yes}"`, as the frame
    /// draws it -- a click anywhere on the word answers it, not only on the
    /// letter.
    pub yes: (u16, u16),
    /// Column span `[start, end)` on `footer_y` of `"n {no}"`.
    pub no: (u16, u16),
    /// The bottom border's row: `rect.y + rect.height - 1`.
    pub footer_y: u16,
}

/// The bottom border's text, before the frame wraps it in its own surrounding
/// spaces: `"y {yes} \u{b7} n {no}"`.
///
/// A function rather than a field on [`Layout`] because [`render`] needs the
/// same string to hand [`overlay::render`] as [`layout`] needs to place the
/// answer spans within it, and a string built in two places is one that can
/// drift.
pub fn footer(c: &Confirm) -> String {
    format!("y {} \u{b7} n {}", c.yes, c.no)
}

/// Where the box goes, sized to its body: `None` when `area` cannot fit even
/// the smallest box this dialogue draws.
///
/// The sizing is circular in one direction only -- the width
/// [`overlay::rect`] picks does not depend on how tall the box ends up, but
/// how tall the box ends up depends on how many rows the body wraps to at
/// that width. So the width is asked for first, with a placeholder `want_h`,
/// the body is wrapped to it, and only then is the real rect asked for with
/// the height the wrapped body actually needs.
pub fn layout(area: Rect, c: &Confirm) -> Option<Layout> {
    let probe = overlay::rect(area, (24, 56), 4, 4, Anchor::Centre);
    if probe.height < 4 {
        return None;
    }
    let probe_inner = overlay::inner(probe);
    // One column of breathing room to the left of the body text -- render
    // draws it at `inner.x + 1`, so wrapping has to leave that column free
    // rather than fill the whole inner width and run into the border.
    let wrap_w = probe_inner.width.saturating_sub(1);
    if wrap_w == 0 {
        return None;
    }

    let mut body_rows = Vec::new();
    for line in &c.body {
        if line.is_empty() {
            body_rows.push(String::new());
            continue;
        }
        for row in wrap::wrap(line, wrap_w) {
            body_rows.push(row.drawn(line).to_string());
        }
    }

    let want_h = body_rows.len() as u16 + 2;
    let rect = overlay::rect(area, (24, 56), want_h, 4, Anchor::Centre);
    let inner = overlay::inner(rect);
    body_rows.truncate(inner.height as usize);

    let footer_text = footer(c);
    let drawn = format!(" {footer_text} ");
    let drawn_w = wrap::width_of(&drawn);
    // The frame right-aligns `drawn` inside the bottom border, so its last
    // character sits one cell clear of the corner.
    let end_x = rect.x + rect.width.saturating_sub(2);
    let drawn_start = (end_x + 1).saturating_sub(drawn_w);
    let footer_start = drawn_start + 1; // past the frame's own leading space

    let yes_label = format!("y {}", c.yes);
    let yes_w = wrap::width_of(&yes_label);
    let yes = (footer_start, footer_start + yes_w);

    let sep_w = wrap::width_of(" \u{b7} ");
    let no_label = format!("n {}", c.no);
    let no_w = wrap::width_of(&no_label);
    let no_start = yes.1 + sep_w;
    let no = (no_start, no_start + no_w);

    let footer_y = rect.y + rect.height - 1;

    Some(Layout {
        rect,
        inner,
        body_rows,
        yes,
        no,
        footer_y,
    })
}

/// Draw the box, its title, its body and its answers.
///
/// Nothing is drawn when [`layout`] returns `None` -- the terminal is too
/// small for this dialogue to be readable at all, and an overlay that cannot
/// be read should not be able to trap its reader behind it either, so a
/// caller pairs this with a key path that still answers `Esc` regardless of
/// whether anything is on screen.
pub fn render(area: Rect, buf: &mut Buffer, t: &Theme, c: &Confirm) {
    let Some(l) = layout(area, c) else {
        return;
    };
    let footer_text = footer(c);
    overlay::render(
        l.rect,
        buf,
        &overlay::Overlay {
            theme: t,
            title: &c.title,
            detail: None,
            footer: Some(&footer_text),
        },
    );

    for (i, row) in l.body_rows.iter().enumerate() {
        buf.set_string(
            l.inner.x + 1,
            l.inner.y + i as u16,
            row,
            Style::default().fg(rgb(t.fg)),
        );
    }
}

/// What a key did to the dialogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    /// Anything else, `Enter` included -- a modal takes every key; it only
    /// acts on the ones that answer it.
    Waiting,
    /// Quitting works from inside a confirm dialogue the same as it does
    /// everywhere else in the program.
    Quit,
}

/// Read one key as an answer.
///
/// `y`/`Y` says yes; `n`/`N`/`Esc` says no, because backing out should never
/// cost more than one obvious key. `Enter` is deliberately not among either
/// -- see the module doc for why -- so it falls through to `Waiting` with
/// every other key that is not this dialogue's business.
pub fn answer(key: KeyEvent) -> Answer {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Answer::Quit,
            _ => Answer::Waiting,
        };
    }
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => Answer::Yes,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Answer::No,
        _ => Answer::Waiting,
    }
}

/// Read a click against a layout already computed for the same box.
///
/// `None` off the footer row, and `None` on the footer row but off both
/// words -- a click on the title, the body, or the blank stretch of border
/// between the two answers neither confirms nor dismisses the dialogue. A
/// question this consequential should not be answerable by a stray click
/// near where the answer happens to sit.
pub fn hit(l: &Layout, x: u16, y: u16) -> Option<Answer> {
    if y != l.footer_y {
        return None;
    }
    if x >= l.yes.0 && x < l.yes.1 {
        return Some(Answer::Yes);
    }
    if x >= l.no.0 && x < l.no.1 {
        return Some(Answer::No);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::test_theme;
    use super::*;

    fn confirm() -> Confirm {
        Confirm {
            title: "delete".into(),
            body: vec!["this cannot be undone.".into()],
            yes: "delete",
            no: "keep",
        }
    }

    #[test]
    fn layout_wraps_the_body_to_the_box() {
        let c = Confirm {
            body: vec!["word ".repeat(40).trim_end().to_string()],
            ..confirm()
        };
        let area = Rect::new(0, 0, 60, 20);
        let l = layout(area, &c).expect("fits");
        assert!(l.body_rows.len() > 1, "{:?}", l.body_rows);
        for row in &l.body_rows {
            assert!(
                wrap::width_of(row) < l.inner.width,
                "{row:?} is not narrower than {}",
                l.inner.width
            );
        }
    }

    #[test]
    fn the_box_grows_with_the_body_and_stops_at_the_terminal() {
        let area = Rect::new(0, 0, 60, 20);
        let c = confirm();
        let l = layout(area, &c).expect("fits");
        assert_eq!(
            l.rect.height, 4,
            "1 body row + 2 borders is 3, so min_h 4 wins"
        );

        let long_body: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        let c = Confirm {
            body: long_body,
            ..confirm()
        };
        let l = layout(area, &c).expect("fits");
        assert!(
            l.rect.height <= area.height - overlay::MARGIN_Y,
            "the box grew past the terminal's own margin: {}",
            l.rect.height
        );
        assert!(
            l.body_rows.len() < 30,
            "the body should have been truncated to fit: {}",
            l.body_rows.len()
        );
    }

    #[test]
    fn render_puts_the_verbs_on_the_bottom_border() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let c = confirm();
        render(area, &mut buf, &theme, &c);
        let l = layout(area, &c).unwrap();

        let bottom: String = (l.rect.x..l.rect.x + l.rect.width)
            .map(|x| buf[(x, l.footer_y)].symbol().to_string())
            .collect();
        assert!(bottom.contains(" y delete \u{b7} n keep "), "{bottom:?}");

        let top: String = (l.rect.x..l.rect.x + l.rect.width)
            .map(|x| buf[(x, l.rect.y)].symbol().to_string())
            .collect();
        assert!(top.contains("\u{2550} DELETE "), "{top:?}");
    }

    #[test]
    fn a_click_on_yes_answers_yes_on_no_answers_no_and_elsewhere_nothing() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let c = confirm();
        render(area, &mut buf, &theme, &c);
        let l = layout(area, &c).unwrap();

        let spelled: String = (l.yes.0..l.yes.1)
            .map(|x| buf[(x, l.footer_y)].symbol().to_string())
            .collect();
        assert_eq!(spelled, "y delete");

        let spelled: String = (l.no.0..l.no.1)
            .map(|x| buf[(x, l.footer_y)].symbol().to_string())
            .collect();
        assert_eq!(spelled, "n keep");

        assert_eq!(hit(&l, l.yes.0, l.footer_y), Some(Answer::Yes));
        assert_eq!(hit(&l, l.yes.1 - 1, l.footer_y), Some(Answer::Yes));
        assert_eq!(hit(&l, l.no.0, l.footer_y), Some(Answer::No));
        assert_eq!(
            hit(&l, l.rect.x, l.rect.y),
            None,
            "a click on the title does nothing"
        );
        assert_eq!(
            hit(&l, l.yes.1, l.footer_y),
            None,
            "a click one past the word misses"
        );
    }

    #[test]
    fn keys_answer_as_documented() {
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(answer(key('y')), Answer::Yes);
        assert_eq!(answer(key('Y')), Answer::Yes);
        assert_eq!(answer(key('n')), Answer::No);
        assert_eq!(answer(key('N')), Answer::No);
        assert_eq!(
            answer(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Answer::No
        );
        assert_eq!(
            answer(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Answer::Waiting,
            "Enter is deliberately not bound to yes"
        );
        assert_eq!(
            answer(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Answer::Quit
        );
        assert_eq!(answer(key('x')), Answer::Waiting);
    }

    #[test]
    fn too_small_an_area_is_none() {
        let c = confirm();
        assert!(layout(Rect::new(0, 0, 10, 3), &c).is_none());
    }
}
