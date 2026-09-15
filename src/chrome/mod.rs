//! The parts of a panel that are not its contents.
//!
//! A border with a title on it, one colour all the way round, a row of action
//! words at the top, and the overlay a panel opens to change its own
//! settings. None of it knows what the panel is for, which is why it is here
//! rather than in either application.
//!
//! Everything in here takes `&`[`Theme`](crate::theme::Theme) -- the core one.
//! An application's own theme derefs to it, so the call sites read the same as
//! they did when this was theirs.

pub mod confirm;
pub mod frame;
pub mod header;
pub mod overlay;
pub mod scrollbar;
pub mod settings;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// A ratatui colour from a theme one.
///
/// Every panel in every application wants this and each used to write its own;
/// it is here so that none of them does again.
pub fn rgb(c: crate::theme::color::Rgb) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(c.r, c.g, c.b)
}

/// One dim line, centred in the middle row of an empty panel: `no results`,
/// `nothing playing`, `queue is empty`.
///
/// The middle row rather than the top, since a panel with nothing in it is
/// not a list with one short entry -- the empty state is the whole content,
/// and it reads as an answer rather than a stray line if it sits where a
/// glance at the panel lands.
pub fn empty(area: Rect, buf: &mut Buffer, t: &crate::theme::Theme, text: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.y + area.height / 2;
    let text = crate::text::fit(text, area.width);
    let text = text.trim_end();
    let x = area.x + area.width.saturating_sub(crate::wrap::width_of(text)) / 2;
    buf.set_string(
        x,
        y,
        text,
        ratatui::style::Style::default().fg(rgb(t.empty_fg)),
    );
}

/// A built-in theme, resolved through the core, for the tests in this module.
#[cfg(test)]
pub(crate) fn test_theme(id: &str) -> crate::theme::Theme {
    use crate::theme::ThemeFile;
    let b = crate::theme::BUILTINS
        .iter()
        .find(|b| b.id == id)
        .unwrap_or_else(|| panic!("no built-in {id}"));
    crate::theme::Theme::resolve(&ThemeFile::parse(b.toml).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_message_is_centred_on_the_middle_row() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 20, 7);
        let mut buf = Buffer::empty(area);
        empty(area, &mut buf, &theme, "nothing playing");

        let row: String = (0..area.width)
            .map(|x| buf[(x, area.y + area.height / 2)].symbol().to_string())
            .collect();
        assert_eq!(row.trim(), "nothing playing");

        // Nothing was drawn on any other row.
        for y in 0..area.height {
            if y == area.y + area.height / 2 {
                continue;
            }
            let row: String = (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            assert_eq!(row.trim(), "", "row {y} should be blank: {row:?}");
        }

        let text_width = crate::wrap::width_of("nothing playing");
        let left_gap = row.chars().take_while(|c| *c == ' ').count() as u16;
        let expected = (area.width.saturating_sub(text_width)) / 2;
        assert_eq!(left_gap, expected, "the message is not centred: {row:?}");
    }
}
