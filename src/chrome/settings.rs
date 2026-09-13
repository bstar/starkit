//! A panel's settings, listed and changed in place.
//!
//! One overlay, given rows by whichever panel opened it. Each panel offers
//! what it actually controls rather than the whole application's surface: the
//! album its artwork, the playlist its order, the equalizer its bands. A
//! setting is easiest to find beside the thing it changes.
//!
//! The same shape as the cover chooser next door -- a centred box, a label
//! column, values right-aligned -- because they are the same kind of list and
//! learning one should teach the other.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::text::truncate;
use crate::theme::color::Rgb;
use crate::theme::Theme;

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// One line: a setting and where it stands, or an action with nothing to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    /// Empty for a row that does something rather than holding a value.
    pub value: String,
}

impl Row {
    pub fn setting(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }

    /// A row that acts when chosen. The ellipsis says it leads somewhere.
    pub fn action(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: String::new(),
        }
    }
}

/// Where the overlay lands, so a click can be tested against it.
pub fn rect(area: Rect, rows: usize) -> Rect {
    let w = area.width.saturating_sub(4).clamp(24, 52);
    let h = area.height.saturating_sub(4).min(rows as u16 + 4).max(6);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// The rows themselves, inside the frame and below the heading.
pub fn list_rect(area: Rect, rows: usize) -> Rect {
    let r = rect(area, rows);
    Rect {
        x: r.x + 1,
        y: r.y + 2,
        width: r.width.saturating_sub(2),
        height: r.height.saturating_sub(3),
    }
}

pub fn clamp_scroll(cursor: usize, scroll: usize, height: usize) -> usize {
    crate::list::clamp_scroll(cursor, scroll, height)
}

/// Which row is at a point, given the same `rows` and `scroll` the overlay was
/// drawn with.
///
/// The inverse of the loop in `render`, and beside it for the reason every
/// panel in both applications keeps its geometry in one function: a row that
/// scrolled off the top is still under the pointer's arithmetic if the mouse
/// side subtracts a differently sized heading. `None` for a click on the
/// frame, on the heading, or past the last row -- an overlay swallows the
/// click either way, but it does not act on it.
pub fn hit(area: Rect, rows: usize, scroll: usize, x: u16, y: u16) -> Option<usize> {
    if rows == 0 {
        return None;
    }
    let list = list_rect(area, rows);
    if x < list.x || x >= list.x + list.width || y < list.y || y >= list.y + list.height {
        return None;
    }
    let index = scroll + usize::from(y - list.y);
    (index < rows).then_some(index)
}

pub struct SettingsView<'a> {
    pub theme: &'a Theme,
    /// What kind of list this is, across the top of the frame.
    pub heading: &'a str,
    /// Which panel's settings these are.
    pub title: &'a str,
    pub rows: &'a [Row],
    pub cursor: usize,
    pub scroll: usize,
}

impl<'a> Widget for SettingsView<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        let r = rect(area, self.rows.len());
        Clear.render(r, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(rgb(t.border_focused)))
            .title(Span::styled(
                format!("{}{} ", super::frame::TITLE_LEAD, self.heading),
                Style::default().fg(rgb(t.header_fg)),
            ))
            .title_bottom(
                Line::from(Span::styled(
                    " enter change \u{b7} esc close ",
                    Style::default().fg(rgb(t.dim)),
                ))
                .right_aligned(),
            )
            .style(Style::default().bg(rgb(t.panel_bg)));
        let inner = block.inner(r);
        block.render(r, buf);
        super::frame::render_corners(r, buf, t, true);

        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let w = inner.width as usize;

        // Whose settings these are. A list of switches with no subject is a
        // list of switches.
        buf.set_string(
            inner.x,
            inner.y,
            truncate(self.title, w),
            Style::default().fg(rgb(t.row_meta_fg)),
        );

        let list = list_rect(area, self.rows.len());
        if self.rows.is_empty() {
            buf.set_string(
                list.x,
                list.y,
                truncate("nothing to change here", w),
                Style::default().fg(rgb(t.empty_fg)),
            );
            return;
        }

        let height = list.height as usize;
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(height) {
            let y = list.y + (i - self.scroll) as u16;
            let selected = i == self.cursor;
            let style = if selected {
                Style::default()
                    .fg(rgb(t.row_selected_fg))
                    .bg(rgb(t.row_selected_bg))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(rgb(t.row_fg))
            };

            // The whole width, so the selection is a bar rather than a
            // highlight around the text.
            buf.set_string(list.x, y, " ".repeat(list.width as usize), style);

            let value_w = row.value.chars().count().min(list.width as usize / 2);
            let label_w = (list.width as usize).saturating_sub(value_w + 3);
            buf.set_string(list.x + 1, y, truncate(&row.label, label_w), style);
            if value_w > 0 {
                let value = truncate(&row.value, value_w);
                let x = list.x + list.width - value.chars().count() as u16 - 1;
                let value_style = if selected {
                    style
                } else {
                    Style::default().fg(rgb(t.accent))
                };
                buf.set_string(x, y, value, value_style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_theme;
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            Row::setting("cover graphics", "auto"),
            Row::setting("fetch cover art", "on"),
            Row::action("choose cover\u{2026}"),
        ]
    }

    fn draw(rows: &[Row], cursor: usize, w: u16, h: u16) -> Vec<String> {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        SettingsView {
            theme: &theme,
            heading: "SETTINGS",
            title: "album",
            rows,
            cursor,
            scroll: 0,
        }
        .render(area, &mut buf);
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn every_setting_is_shown_with_where_it_stands() {
        let all = draw(&rows(), 0, 60, 14).join("\n");
        assert!(all.contains("album"), "no subject: {all}");
        for (label, value) in [("cover graphics", "auto"), ("fetch cover art", "on")] {
            assert!(all.contains(label), "{label} is missing: {all}");
            assert!(all.contains(value), "{value} is missing: {all}");
        }
        assert!(all.contains("choose cover"), "{all}");
    }

    #[test]
    fn the_overlay_stays_inside_the_terminal() {
        for (w, h) in [(40u16, 10u16), (80, 24), (200, 60), (24, 8)] {
            let area = Rect::new(0, 0, w, h);
            let r = rect(area, 12);
            assert!(r.x + r.width <= w, "{w}x{h} overflows across");
            assert!(r.y + r.height <= h, "{w}x{h} overflows down");
            let l = list_rect(area, 12);
            assert!(l.y + l.height <= r.y + r.height, "{w}x{h}: rows escape");
            assert!(
                l.x + l.width <= r.x + r.width,
                "{w}x{h}: rows escape across"
            );
        }
    }

    #[test]
    fn the_selected_row_is_a_bar_across_the_list() {
        let theme = test_theme("cosmic");
        let area = Rect::new(0, 0, 60, 14);
        let mut buf = Buffer::empty(area);
        let rows = rows();
        SettingsView {
            theme: &theme,
            heading: "SETTINGS",
            title: "album",
            rows: &rows,
            cursor: 1,
            scroll: 0,
        }
        .render(area, &mut buf);

        let list = list_rect(area, rows.len());
        let selected = buf[(list.x, list.y + 1)].style().bg;
        assert_eq!(
            selected,
            Some(Color::Rgb(
                theme.row_selected_bg.r,
                theme.row_selected_bg.g,
                theme.row_selected_bg.b
            ))
        );
        assert_ne!(buf[(list.x, list.y)].style().bg, selected);
    }

    /// Every row the overlay drew is under the pointer where it was drawn, and
    /// nothing else is. Checked against the drawing rather than against the
    /// arithmetic, which is the only way the two can be said to agree.
    #[test]
    fn a_click_lands_on_the_row_that_was_drawn_there() {
        let rows = rows();
        let area = Rect::new(0, 0, 60, 14);
        let list = list_rect(area, rows.len());
        for (i, row) in rows.iter().enumerate() {
            let y = list.y + i as u16;
            assert_eq!(hit(area, rows.len(), 0, list.x, y), Some(i));
            let drawn = &draw(&rows, 0, 60, 14)[usize::from(y)];
            assert!(drawn.contains(&row.label), "row {i} is not at {y}: {drawn}");
        }
        // The frame, the heading, and the empty rows below the last one.
        assert_eq!(hit(area, rows.len(), 0, list.x, list.y - 1), None);
        assert_eq!(hit(area, rows.len(), 0, list.x, area.y), None);
        assert_eq!(
            hit(area, rows.len(), 0, list.x, list.y + rows.len() as u16),
            None
        );
        // And across, outside the box.
        assert_eq!(hit(area, rows.len(), 0, 0, list.y), None);
        assert_eq!(hit(area, rows.len(), 0, area.width - 1, list.y), None);
        assert_eq!(hit(area, 0, 0, list.x, list.y), None, "nothing to change");
    }

    /// A scrolled list hits the row that is on screen, not the one that would
    /// have been there before it scrolled.
    #[test]
    fn scrolling_moves_what_is_under_the_pointer() {
        let area = Rect::new(0, 0, 60, 8);
        let list = list_rect(area, 20);
        assert_eq!(hit(area, 20, 7, list.x, list.y), Some(7));
        assert_eq!(hit(area, 20, 7, list.x, list.y + 1), Some(8));
        assert_eq!(hit(area, 20, 19, list.x, list.y + 1), None, "past the end");
    }

    #[test]
    fn a_panel_with_nothing_to_change_says_so() {
        let all = draw(&[], 0, 60, 12).join("\n");
        assert!(all.contains("nothing to change"), "{all}");
    }
}
