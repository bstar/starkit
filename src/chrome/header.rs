//! The row of actions at the top of a docked panel.
//!
//! Words, right-aligned on the first row inside the border. They replace the
//! single `X` that used to sit on the border itself, which took several
//! attempts to get right: a lone glyph is at the mercy of whichever font the
//! terminal falls back to for it, at whatever size and weight that font happens
//! to have. Words are drawn in the same face as the text beside them and cannot
//! go wrong that way.
//!
//! Same doctrine as the rest of the panel chrome: one function decides where
//! the words are, the renderer draws from it and the mouse tests against it.
//! A hit box computed separately from what it points at is the bug this
//! arrangement makes impossible.
//!
//! Which words a panel offers is the application's business, so everything here
//! is generic over [`Word`] -- an enum the application owns, which knows how to
//! say itself. Only the width of the word matters here.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};

use crate::theme::color::Rgb;
use crate::theme::Theme;

fn rgb(c: Rgb) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(c.r, c.g, c.b)
}

/// One action a panel's header offers.
///
/// `Cow` rather than `&'static str` because a word may carry a count, and a
/// count that is not on the word is a count nobody can see. `Copy` because the
/// word list is passed around by value and a header has at most a handful of
/// entries.
pub trait Word: Copy {
    /// The word as drawn.
    fn word(self) -> Cow<'static, str>;

    /// Columns it takes.
    ///
    /// Characters rather than display width: these are an application's own
    /// short labels, not somebody else's text.
    fn width(self) -> u16 {
        self.word().chars().count() as u16
    }
}

/// Rows the header costs a panel.
pub const ROWS: u16 = 1;

/// Blank columns between words.
const GAP: u16 = 2;

/// Blank columns kept to the right, matching the right padding of the lists
/// below so the words line up with what is under them.
const RIGHT_PAD: u16 = 1;

/// The header row: the first row inside the border.
pub fn rect(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Rect {
        height: inner.height.min(ROWS),
        ..inner
    }
}

/// What the panel has left for its own content.
///
/// Every panel takes its content area from here rather than from
/// `block.inner(area)`, so the one-row offset lives in one place. Three
/// separate derivations of the playlist's offset is exactly how a click comes
/// to select the row above the one it landed on.
pub fn body(area: Rect) -> Rect {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    // A panel with no room for a body gets an empty rect inside itself rather
    // than one starting past its own bottom edge. `Block::inner` moves the
    // corner down whether or not there was anything to move it into.
    let bottom = area.y.saturating_add(area.height);
    Rect {
        y: inner.y.saturating_add(ROWS).min(bottom),
        height: inner.height.saturating_sub(ROWS),
        ..inner
    }
}

/// Columns a run of words needs, including the gaps between them.
fn width_of<I: Word>(items: &[I]) -> u16 {
    let words: u16 = items.iter().map(|i| i.width()).sum();
    words + GAP * items.len().saturating_sub(1) as u16
}

/// Where each word sits, right to left; empty when none of them fit.
///
/// Words are dropped from the left until the rest fit, rather than the header
/// vanishing whole. A panel too narrow for `filter settings close` still has
/// room for `close`, and losing the way to close a panel because it got narrow
/// would be a worse answer than losing the way to reorder it.
///
/// The renderer and the mouse handler both come through here, so a word that
/// was never drawn cannot be clicked.
pub fn slots<I: Word>(area: Rect, items: &[I]) -> Vec<(I, Rect)> {
    let row = rect(area);
    if row.height == 0 {
        return Vec::new();
    }
    // A leading column as well, so the words never run into the left border.
    let mut kept = items;
    while !kept.is_empty() && row.width < 1 + width_of(kept) + RIGHT_PAD {
        kept = &kept[1..];
    }
    if kept.is_empty() {
        // Nothing fits, and the walk below would step off the left edge of a
        // panel too narrow to hold even the padding.
        return Vec::new();
    }

    let mut out = Vec::with_capacity(kept.len());
    let mut right = row.x + row.width - RIGHT_PAD;
    for item in kept.iter().rev() {
        right -= item.width();
        out.push((
            *item,
            Rect {
                x: right,
                y: row.y,
                width: item.width(),
                height: 1,
            },
        ));
        right = right.saturating_sub(GAP);
    }
    out.reverse();
    out
}

/// Which word a click landed on, if any.
pub fn hit<I: Word>(area: Rect, items: &[I], x: u16, y: u16) -> Option<I> {
    slots(area, items)
        .into_iter()
        .find(|(_, r)| y == r.y && x >= r.x && x < r.x + r.width)
        .map(|(item, _)| item)
}

/// Draw the header.
///
/// In `t.dim`, the same weight as the track count that used to sit beside the
/// close mark. Chrome should read as chrome.
pub fn render<I: Word>(area: Rect, items: &[I], buf: &mut Buffer, t: &Theme) {
    let style = Style::default().fg(rgb(t.dim));
    for (item, r) in slots(area, items) {
        buf.set_string(r.x, r.y, item.word().as_ref(), style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::Widget;

    /// A panel's words, as an application would write them: a couple of fixed
    /// ones and one that carries a count, since the count is what makes the
    /// width move under the hit boxes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Item {
        Filter,
        Settings,
        Close,
        Tagged(usize),
    }

    impl Word for Item {
        fn word(self) -> Cow<'static, str> {
            match self {
                Item::Filter => "sorting".into(),
                Item::Settings => "settings".into(),
                Item::Close => "close".into(),
                Item::Tagged(n) => format!("{n} tagged").into(),
            }
        }
    }

    const PLAIN: &[Item] = &[Item::Settings, Item::Close];
    const WITH_FILTER: &[Item] = &[Item::Filter, Item::Settings, Item::Close];

    /// Draw a panel with a header and return its rows as text.
    fn draw(w: u16, h: u16, items: &[Item]) -> Vec<String> {
        let theme = super::super::test_theme("cosmic");
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        Block::default()
            .borders(Borders::ALL)
            .render(area, &mut buf);
        render(area, items, &mut buf, &theme);
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn under(rows: &[String], r: Rect) -> String {
        rows[r.y as usize]
            .chars()
            .skip(r.x as usize)
            .take(r.width as usize)
            .collect()
    }

    #[test]
    fn each_word_is_where_its_hit_box_says_it_is() {
        // The whole reason both come from `slots`. A click that misses what it
        // is pointing at is worse than no click at all.
        for items in [PLAIN, WITH_FILTER] {
            for (w, h) in [(40u16, 9u16), (60, 9), (100, 24), (24, 9)] {
                let area = Rect::new(0, 0, w, h);
                let rows = draw(w, h, items);
                for (item, r) in slots(area, items) {
                    assert_eq!(under(&rows, r), item.word(), "{items:?} at {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn a_click_on_a_word_reports_that_word() {
        let area = Rect::new(0, 0, 60, 9);
        let placed = slots(area, WITH_FILTER);
        assert_eq!(placed.len(), 3, "all three fit at 60 columns");
        for (item, r) in &placed {
            assert_eq!(hit(area, WITH_FILTER, r.x, r.y), Some(*item));
            assert_eq!(hit(area, WITH_FILTER, r.x + r.width - 1, r.y), Some(*item));
            // The gap in front of it belongs to nothing.
            assert_eq!(hit(area, WITH_FILTER, r.x - 1, r.y), None);
            // Nor does the row below.
            assert_eq!(hit(area, WITH_FILTER, r.x, r.y + 1), None);
        }
        assert_eq!(hit(area, WITH_FILTER, area.x + 1, placed[0].1.y), None);
        // And a panel that does not offer `sorting` does not answer for it.
        assert_ne!(
            hit(area, PLAIN, placed[0].1.x, placed[0].1.y),
            Some(Item::Filter)
        );
    }

    #[test]
    fn a_narrow_panel_keeps_the_last_word_and_drops_the_rest() {
        // The header used to vanish whole below its full width. Losing the way
        // to close a panel because it got narrow is a worse answer than losing
        // the way to reorder it.
        let mut seen: Vec<usize> = Vec::new();
        for w in 0..=50u16 {
            let area = Rect::new(0, 0, w, 9);
            let placed = slots(area, WITH_FILTER);
            seen.push(placed.len());
            let rows = draw(w, 9, WITH_FILTER);
            let all = rows.join("");
            for item in WITH_FILTER {
                let drawn = placed.iter().any(|(i, _)| i == item);
                assert_eq!(
                    all.contains(item.word().as_ref()),
                    drawn,
                    "{:?} at width {w}",
                    item.word()
                );
            }
            // Whatever survives, the rightmost word is in it.
            if !placed.is_empty() {
                assert_eq!(placed.last().unwrap().0, Item::Close, "at width {w}");
            }
            // And nothing is claimed that was not drawn.
            for x in 0..w {
                if let Some(item) = hit(area, WITH_FILTER, x, 1) {
                    assert!(placed.iter().any(|(i, _)| *i == item), "at width {w}");
                }
            }
        }
        assert!(
            seen.contains(&0) && seen.contains(&1) && seen.contains(&2) && seen.contains(&3),
            "every step of the ladder should be reachable: {seen:?}"
        );
    }

    #[test]
    fn a_word_that_carries_a_count_is_measured_as_drawn() {
        // The counts change the widths, so this is the arrangement most able
        // to put a hit box beside its word rather than on it.
        let theme = super::super::test_theme("cosmic");
        for tagged in [1usize, 9, 42, 793] {
            let words = [Item::Tagged(tagged), Item::Settings, Item::Close];
            let area = Rect::new(0, 0, 100, 6);
            let mut buf = Buffer::empty(area);
            render(area, &words, &mut buf, &theme);
            for (item, r) in slots(area, &words) {
                let drawn: String = (0..r.width)
                    .map(|dx| buf[(r.x + dx, r.y)].symbol().to_string())
                    .collect();
                assert_eq!(drawn, item.word().as_ref(), "{item:?} at {tagged} tagged");
                assert_eq!(hit(area, &words, r.x, r.y), Some(item));
            }
        }
    }

    #[test]
    fn the_body_starts_below_the_header() {
        let area = Rect::new(0, 0, 60, 9);
        let inner = Block::default().borders(Borders::ALL).inner(area);
        let b = body(area);
        assert_eq!(b.y, inner.y + ROWS);
        assert_eq!(b.height, inner.height - ROWS);
        assert_eq!(b.x, inner.x);
        assert_eq!(b.width, inner.width);
        // And the header sits exactly where the body is not.
        assert_eq!(rect(area).y + rect(area).height, b.y);
    }

    #[test]
    fn a_panel_with_no_room_has_no_body_rather_than_a_wrapped_one() {
        for h in 0..=3u16 {
            let area = Rect::new(0, 0, 60, h);
            let b = body(area);
            let inner = Block::default().borders(Borders::ALL).inner(area);
            assert!(
                b.height <= inner.height,
                "body taller than the panel at {h}"
            );
            assert!(
                b.y + b.height <= area.y + area.height,
                "body escapes at {h}"
            );
        }
    }
}
