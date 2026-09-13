//! A text field.
//!
//! Both applications had the same placeholder: a `String`, a fake caret drawn
//! as `▂`, and whatever editing keys somebody needed that week. This is the
//! real thing -- one line or several, a cursor that survives CJK and emoji,
//! the readline keys people have in their fingers, and a `render` that reports
//! where the terminal's own cursor should go so the caret is the real one.
//!
//! ## The cursor
//!
//! The cursor is a byte offset into the text and is always on a character
//! boundary, so `&text[..cursor]` never panics. Movement and deletion step by
//! *cluster* -- see [`crate::wrap`] -- so backspacing a family emoji removes
//! the family rather than one of its parents.
//!
//! ## Words
//!
//! The word keys break on `char::is_alphanumeric`, which is a Unicode
//! property and covers every script's letters and digits, but knows nothing
//! about where a word ends in a script that does not use spaces: `Alt+B` in a
//! line of Japanese jumps to the start of the run, not to the start of the
//! last word. Doing better means a segmenter, its tables, and a dictionary for
//! the languages where even the segmenter guesses. The keys people use these
//! on are `Ctrl+W` to take back the thing they just typed and `Alt+B` to go
//! back over it, and for that this is the right amount of machinery.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::wrap::{self, next_boundary, prev_boundary, width_of, Row};

/// What a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// The field handled it; redraw.
    Consumed,
    /// Enter: the caller sends, runs, or accepts whatever this field is for.
    Submit,
    /// Escape.
    Cancel,
    /// Not a key this field knows. The caller's own key handling gets it,
    /// which is what lets `alt+m` toggle a panel while the composer has
    /// focus.
    Ignored,
}

/// An editable string, with a cursor.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    text: String,
    cursor: usize,
    multiline: bool,
    max_chars: Option<usize>,
    /// First visible row, for a multi-line field taller than its rect.
    scroll: u16,
    /// First visible column, for a single-line field longer than its rect.
    hscroll: u16,
    /// The width the last `render` used, so that up and down know what the
    /// wrapped lines are. Before the first render there are none, and up and
    /// down fall back to the lines the text actually has.
    width: u16,
    /// The column an unbroken run of up and down presses is aiming for, so
    /// that passing through a short line does not lose the position.
    goal: Option<u16>,
}

impl TextInput {
    /// A one-line field. Enter submits; a pasted newline becomes a space.
    pub fn single() -> Self {
        Self::default()
    }

    /// A field that can hold several lines. Enter still submits -- it is a
    /// message composer, not an editor -- and `shift+enter` or `alt+enter`
    /// breaks the line.
    pub fn multiline() -> Self {
        Self {
            multiline: true,
            ..Self::default()
        }
    }

    /// Refuse input past this many characters. Characters, not bytes: the
    /// limits this exists for are message lengths, and those are counted the
    /// way a person would count them.
    pub fn with_max_chars(mut self, n: usize) -> Self {
        self.max_chars = Some(n);
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.set_text(text);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The cursor as a byte offset, always on a character boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// Replace the text, putting the cursor at the end -- which is where it
    /// belongs when a draft is restored or a message is opened for editing.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.scroll = 0;
        self.hscroll = 0;
        self.goal = None;
    }

    /// Take the text and leave the field empty.
    pub fn take(&mut self) -> String {
        let out = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal = None;
        out
    }

    pub fn clear(&mut self) {
        self.take();
    }

    /// How many rows the text needs at this width. A composer sizes itself
    /// with this.
    pub fn height(&self, width: u16) -> u16 {
        if !self.multiline {
            return 1;
        }
        wrap::height(&self.text, width)
    }

    /// Insert text from the clipboard or a bracketed paste.
    ///
    /// `\r` goes: a paste from a Windows clipboard or an email carries them,
    /// and a lone `\r` in a terminal buffer moves the cursor to the start of
    /// the line rather than printing. In a one-line field newlines become
    /// spaces, because the alternative is silently dropping half of what was
    /// pasted.
    pub fn paste(&mut self, s: &str) {
        let mut clean = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '\r' => {}
                '\n' if !self.multiline => clean.push(' '),
                c => clean.push(c),
            }
        }
        self.insert_str(&clean);
    }

    /// Handle a key. See [`Edit`] for what the answer means.
    pub fn handle(&mut self, key: KeyEvent) -> Edit {
        // Terminals that report key releases send the same key twice.
        if key.kind == KeyEventKind::Release {
            return Edit::Ignored;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Up and down keep aiming at the column they started from; everything
        // else gives up on it.
        let keeps_goal = matches!(key.code, KeyCode::Up | KeyCode::Down);
        if !keeps_goal {
            self.goal = None;
        }

        match key.code {
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.move_to(self.line_start()),
                'e' => self.move_to(self.line_end()),
                'k' => self.delete_range(self.cursor, self.line_end()),
                'u' => self.delete_range(self.line_start(), self.cursor),
                'w' => self.delete_range(self.word_left(), self.cursor),
                _ => return Edit::Ignored,
            },
            KeyCode::Char(c) if alt => match c.to_ascii_lowercase() {
                'b' => self.move_to(self.word_left()),
                'f' => self.move_to(self.word_right()),
                'd' => self.delete_range(self.cursor, self.word_right()),
                // Every other alt key belongs to the application, so that the
                // panel bindings still work while somebody is typing.
                _ => return Edit::Ignored,
            },
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Backspace => {
                let to = prev_boundary(&self.text, self.cursor);
                self.delete_range(to, self.cursor);
            }
            KeyCode::Delete => {
                let to = next_boundary(&self.text, self.cursor);
                self.delete_range(self.cursor, to);
            }
            KeyCode::Left => self.move_to(prev_boundary(&self.text, self.cursor)),
            KeyCode::Right => self.move_to(next_boundary(&self.text, self.cursor)),
            KeyCode::Home => self.move_to(self.line_start()),
            KeyCode::End => self.move_to(self.line_end()),
            KeyCode::Up if self.multiline => self.step_row(-1),
            KeyCode::Down if self.multiline => self.step_row(1),
            KeyCode::Enter if self.multiline && (shift || alt) => self.insert('\n'),
            KeyCode::Enter => return Edit::Submit,
            KeyCode::Esc => return Edit::Cancel,
            _ => return Edit::Ignored,
        }
        Edit::Consumed
    }

    /// Draw the field and say where the terminal cursor goes, or `None` when
    /// the cursor is not on screen -- a zero-sized rect, or a line scrolled
    /// out of view.
    ///
    /// It takes `&mut self` because the scroll that keeps the cursor visible
    /// is decided here: it depends on the width, and the width is not known
    /// until there is a rect.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, style: Style) -> Option<(u16, u16)> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        self.width = area.width;

        if !self.multiline {
            return self.render_single(area, buf, style);
        }

        let rows = wrap::wrap(&self.text, area.width);
        let (row, col) = cursor_cell(&self.text, &rows, self.cursor);
        self.scroll = clamp_scroll(row, self.scroll, area.height);

        let mut cursor = None;
        for (i, r) in rows
            .iter()
            .enumerate()
            .skip(usize::from(self.scroll))
            .take(usize::from(area.height))
        {
            let y = area.y + (i as u16 - self.scroll);
            buf.set_stringn(
                area.x,
                y,
                r.drawn(&self.text),
                usize::from(area.width),
                style,
            );
            if i as u16 == row {
                // A cursor one past the last column of a full row has nowhere
                // to sit; it belongs on the last cell rather than outside.
                cursor = Some((area.x + col.min(area.width - 1), y));
            }
        }
        cursor
    }

    fn render_single(&mut self, area: Rect, buf: &mut Buffer, style: Style) -> Option<(u16, u16)> {
        let col = width_of(&self.text[..self.cursor]);
        if col < self.hscroll {
            self.hscroll = col;
        } else if col >= self.hscroll + area.width {
            self.hscroll = col + 1 - area.width;
        }
        // Start at the first cluster that is fully at or past the scroll, so a
        // wide character is never drawn as its right half.
        let mut start = self.text.len();
        let mut at = 0u16;
        for (i, cl) in wrap::clusters(&self.text) {
            if at >= self.hscroll {
                start = i;
                break;
            }
            at += width_of(cl);
        }
        buf.set_stringn(
            area.x,
            area.y,
            &self.text[start..],
            usize::from(area.width),
            style,
        );
        Some((area.x + (col - self.hscroll).min(area.width - 1), area.y))
    }

    /// Which row and column the cursor is on, at the width of the last
    /// render. Useful for a caller that draws its own caret.
    pub fn cursor_cell(&self) -> (u16, u16) {
        if !self.multiline || self.width == 0 {
            return (0, width_of(&self.text[..self.cursor]));
        }
        cursor_cell(&self.text, &wrap::wrap(&self.text, self.width), self.cursor)
    }

    // -- editing -----------------------------------------------------------

    fn insert(&mut self, c: char) {
        let mut b = [0u8; 4];
        self.insert_str(c.encode_utf8(&mut b));
    }

    fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(max) = self.max_chars {
            let have = self.text.chars().count();
            if have >= max {
                return;
            }
            let room = max - have;
            if s.chars().count() > room {
                let end = s
                    .char_indices()
                    .nth(room)
                    .map(|(i, _)| i)
                    .unwrap_or(s.len());
                self.text.insert_str(self.cursor, &s[..end]);
                self.cursor += end;
                return;
            }
        }
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn move_to(&mut self, at: usize) {
        self.cursor = at.min(self.text.len());
        debug_assert!(self.text.is_char_boundary(self.cursor));
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        let (from, to) = (from.min(to), from.max(to));
        if from == to {
            return;
        }
        self.text.replace_range(from..to, "");
        if self.cursor > to {
            self.cursor -= to - from;
        } else if self.cursor > from {
            self.cursor = from;
        }
    }

    /// The start of the logical line the cursor is on. Logical rather than
    /// wrapped: `Home` in a composer goes to the start of the paragraph, and
    /// a line that happens to be wrapped is still one line to the person who
    /// typed it.
    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len())
    }

    fn word_left(&self) -> usize {
        let mut at = self.cursor;
        while at > 0 {
            let c = self.text[..at].chars().next_back().expect("in bounds");
            if c.is_alphanumeric() {
                break;
            }
            at -= c.len_utf8();
        }
        while at > 0 {
            let c = self.text[..at].chars().next_back().expect("in bounds");
            if !c.is_alphanumeric() {
                break;
            }
            at -= c.len_utf8();
        }
        at
    }

    fn word_right(&self) -> usize {
        let mut at = self.cursor;
        while at < self.text.len() {
            let c = self.text[at..].chars().next().expect("in bounds");
            if c.is_alphanumeric() {
                break;
            }
            at += c.len_utf8();
        }
        while at < self.text.len() {
            let c = self.text[at..].chars().next().expect("in bounds");
            if !c.is_alphanumeric() {
                break;
            }
            at += c.len_utf8();
        }
        at
    }

    /// Move the cursor a row up or down through the wrapped layout.
    fn step_row(&mut self, by: i16) {
        let width = if self.width > 0 { self.width } else { u16::MAX };
        let rows = wrap::wrap(&self.text, width);
        let (row, col) = cursor_cell(&self.text, &rows, self.cursor);
        let goal = *self.goal.get_or_insert(col);

        let next = i32::from(row) + i32::from(by);
        if next < 0 || next as usize >= rows.len() {
            return;
        }
        self.cursor = byte_at_column(&self.text, &rows[next as usize], goal);
    }
}

/// Which row and column of `rows` a byte offset falls on.
fn cursor_cell(text: &str, rows: &[Row], at: usize) -> (u16, u16) {
    for (i, r) in rows.iter().enumerate() {
        // The cursor at the very end of the text belongs on the last row, and
        // a cursor on a break belongs to the row that starts there.
        let last = i + 1 == rows.len();
        if at < r.range.end || (last && at <= r.range.end) {
            let start = r.range.start.min(at);
            return (i as u16, width_of(&text[start..at]));
        }
    }
    (0, 0)
}

/// The byte offset on `row` nearest to display column `col`.
fn byte_at_column(text: &str, row: &Row, col: u16) -> usize {
    let drawn = row.drawn(text);
    let mut at = row.range.start;
    let mut w = 0u16;
    for (i, cl) in wrap::clusters(drawn) {
        if w >= col {
            return row.range.start + i;
        }
        w += width_of(cl);
        at = row.range.start + i + cl.len();
    }
    at
}

/// Keep `cursor` inside a window of `height` rows starting at `scroll`.
///
/// The same arithmetic as `list::clamp_scroll`, in terms of rows rather than
/// items; it is three lines and importing it would tie a text field to a list
/// widget for no reason.
fn clamp_scroll(cursor: u16, scroll: u16, height: u16) -> u16 {
    if height == 0 {
        0
    } else if cursor < scroll {
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
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn typed(i: &mut TextInput, s: &str) {
        for c in s.chars() {
            assert_eq!(i.handle(key(KeyCode::Char(c))), Edit::Consumed);
        }
    }

    /// A string with everything awkward in it: a two-cell CJK character, a
    /// letter with a combining accent, an emoji with a skin tone, and a
    /// zero-width-joiner family.
    const HARD: &str = "a\u{301}字\u{1f44b}\u{1f3fd}\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}z";

    #[test]
    fn typing_puts_the_characters_in_and_moves_the_cursor() {
        let mut i = TextInput::single();
        typed(&mut i, "hello");
        assert_eq!(i.text(), "hello");
        assert_eq!(i.cursor(), 5);
    }

    #[test]
    fn enter_submits_and_escape_cancels() {
        let mut i = TextInput::single();
        assert_eq!(i.handle(key(KeyCode::Enter)), Edit::Submit);
        assert_eq!(i.handle(key(KeyCode::Esc)), Edit::Cancel);
    }

    #[test]
    fn a_one_line_field_does_not_take_a_newline() {
        let mut i = TextInput::single();
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(i.handle(shift_enter), Edit::Submit);
        assert!(i.is_empty());
    }

    #[test]
    fn shift_enter_and_alt_enter_break_a_line_in_a_composer() {
        for m in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            let mut i = TextInput::multiline();
            typed(&mut i, "one");
            assert_eq!(i.handle(KeyEvent::new(KeyCode::Enter, m)), Edit::Consumed);
            typed(&mut i, "two");
            assert_eq!(i.text(), "one\ntwo");
            assert_eq!(i.handle(key(KeyCode::Enter)), Edit::Submit);
        }
    }

    #[test]
    fn unknown_keys_are_left_for_the_application() {
        let mut i = TextInput::multiline();
        assert_eq!(i.handle(alt('m')), Edit::Ignored, "alt+m toggles a panel");
        assert_eq!(i.handle(ctrl('g')), Edit::Ignored, "ctrl+g opens a picker");
        assert_eq!(i.handle(key(KeyCode::F(1))), Edit::Ignored);
        assert!(i.is_empty(), "and none of them typed anything");
    }

    #[test]
    fn a_key_release_is_not_a_second_keypress() {
        let mut i = TextInput::single();
        let mut k = key(KeyCode::Char('x'));
        i.handle(k);
        k.kind = KeyEventKind::Release;
        assert_eq!(i.handle(k), Edit::Ignored);
        assert_eq!(i.text(), "x");
    }

    #[test]
    fn backspace_and_delete_take_one_cluster_at_a_time() {
        let mut i = TextInput::single().with_text(HARD);
        let clusters = wrap::clusters(HARD).count();
        for _ in 0..clusters {
            assert_eq!(i.handle(key(KeyCode::Backspace)), Edit::Consumed);
            assert!(i.text().is_char_boundary(i.cursor()));
        }
        assert!(i.is_empty(), "{clusters} presses emptied it");

        let mut i = TextInput::single().with_text(HARD);
        i.move_to(0);
        for _ in 0..clusters {
            i.handle(key(KeyCode::Delete));
        }
        assert!(i.is_empty());
    }

    #[test]
    fn the_cursor_never_lands_inside_a_character() {
        let mut i = TextInput::single().with_text(HARD);
        i.move_to(0);
        while i.cursor() < HARD.len() {
            let before = i.cursor();
            i.handle(key(KeyCode::Right));
            assert!(HARD.is_char_boundary(i.cursor()));
            assert!(i.cursor() > before, "right always moves");
        }
        while i.cursor() > 0 {
            let before = i.cursor();
            i.handle(key(KeyCode::Left));
            assert!(HARD.is_char_boundary(i.cursor()));
            assert!(i.cursor() < before, "left always moves");
        }
    }

    #[test]
    fn left_and_right_step_over_a_whole_emoji() {
        let mut i = TextInput::single().with_text(HARD);
        i.move_to(0);
        let mut stops = vec![0];
        while i.cursor() < HARD.len() {
            i.handle(key(KeyCode::Right));
            stops.push(i.cursor());
        }
        assert_eq!(stops.len(), wrap::clusters(HARD).count() + 1);
    }

    #[test]
    fn home_and_end_work_on_the_line_the_cursor_is_on() {
        let mut i = TextInput::multiline().with_text("first\nsecond\nthird");
        assert_eq!(i.cursor(), 18);
        i.handle(key(KeyCode::Home));
        assert_eq!(i.cursor(), 13);
        i.handle(key(KeyCode::Up));
        i.handle(key(KeyCode::End));
        assert_eq!(&i.text()[..i.cursor()], "first\nsecond");
        assert_eq!(i.handle(ctrl('a')), Edit::Consumed);
        assert_eq!(i.cursor(), 6);
        i.handle(ctrl('e'));
        assert_eq!(i.cursor(), 12);
    }

    #[test]
    fn ctrl_k_and_ctrl_u_cut_to_the_ends_of_the_line() {
        let mut i = TextInput::single().with_text("hello world");
        i.move_to(6);
        i.handle(ctrl('k'));
        assert_eq!(i.text(), "hello ");
        i.handle(ctrl('u'));
        assert_eq!(i.text(), "");
    }

    #[test]
    fn ctrl_k_stops_at_the_end_of_the_line_not_the_end_of_the_text() {
        let mut i = TextInput::multiline().with_text("one\ntwo");
        i.move_to(0);
        i.handle(ctrl('k'));
        assert_eq!(i.text(), "\ntwo");
    }

    #[test]
    fn ctrl_w_takes_back_the_word_just_typed() {
        let mut i = TextInput::single();
        typed(&mut i, "send this now");
        i.handle(ctrl('w'));
        assert_eq!(i.text(), "send this ");
        i.handle(ctrl('w'));
        assert_eq!(i.text(), "send ");
    }

    #[test]
    fn ctrl_w_over_punctuation_takes_the_punctuation_and_the_word() {
        let mut i = TextInput::single().with_text("hey @someone!!");
        i.handle(ctrl('w'));
        assert_eq!(i.text(), "hey @");
    }

    #[test]
    fn alt_b_and_alt_f_walk_the_words() {
        let mut i = TextInput::single().with_text("one two three");
        i.handle(alt('b'));
        assert_eq!(i.cursor(), 8);
        i.handle(alt('b'));
        assert_eq!(i.cursor(), 4);
        i.handle(alt('f'));
        assert_eq!(i.cursor(), 7);
        i.handle(alt('f'));
        assert_eq!(i.cursor(), 13);
    }

    #[test]
    fn alt_d_deletes_the_word_in_front() {
        let mut i = TextInput::single().with_text("one two three");
        i.move_to(4);
        i.handle(alt('d'));
        assert_eq!(i.text(), "one  three");
    }

    #[test]
    fn word_keys_treat_letters_of_any_script_as_letters() {
        let mut i = TextInput::single().with_text("hello мир");
        i.handle(ctrl('w'));
        assert_eq!(i.text(), "hello ");
    }

    #[test]
    fn up_and_down_move_between_the_typed_lines() {
        let mut i = TextInput::multiline().with_text("alpha\nbeta\ngamma");
        i.render(
            Rect::new(0, 0, 20, 3),
            &mut Buffer::empty(Rect::new(0, 0, 20, 3)),
            Style::default(),
        );
        i.move_to(i.text().len());
        i.handle(key(KeyCode::Up));
        assert_eq!(&i.text()[..i.cursor()], "alpha\nbeta");
        i.handle(key(KeyCode::Down));
        assert_eq!(&i.text()[..i.cursor()], "alpha\nbeta\ngamma");
    }

    #[test]
    fn up_and_down_keep_the_column_across_a_short_line() {
        let mut i = TextInput::multiline().with_text("aaaaaaaa\nbb\ncccccccc");
        let area = Rect::new(0, 0, 20, 3);
        i.render(area, &mut Buffer::empty(area), Style::default());
        i.move_to(6); // column 6 of the first line
        i.handle(key(KeyCode::Down));
        assert_eq!(i.cursor(), 11, "the short line only has two columns");
        i.handle(key(KeyCode::Down));
        assert_eq!(i.cursor(), 18, "and the column comes back on the next");
    }

    #[test]
    fn up_and_down_move_between_wrapped_rows_of_one_line() {
        let mut i = TextInput::multiline().with_text("aaaa bbbb cccc");
        let area = Rect::new(0, 0, 5, 3);
        i.render(area, &mut Buffer::empty(area), Style::default());
        i.move_to(0);
        i.handle(key(KeyCode::Down));
        assert_eq!(i.cursor(), 5, "the second wrapped row");
        i.handle(key(KeyCode::Down));
        assert_eq!(i.cursor(), 10);
        i.handle(key(KeyCode::Up));
        assert_eq!(i.cursor(), 5);
    }

    #[test]
    fn up_at_the_first_row_stays_put() {
        let mut i = TextInput::multiline().with_text("one\ntwo");
        i.move_to(1);
        i.handle(key(KeyCode::Up));
        assert_eq!(i.cursor(), 1);
    }

    #[test]
    fn a_pasted_newline_becomes_a_space_in_a_one_line_field() {
        let mut i = TextInput::single();
        i.paste("one\r\ntwo\r\n");
        assert_eq!(i.text(), "one two ");
    }

    #[test]
    fn a_paste_keeps_its_lines_in_a_composer_but_loses_the_carriage_returns() {
        let mut i = TextInput::multiline();
        i.paste("one\r\ntwo");
        assert_eq!(i.text(), "one\ntwo");
        assert_eq!(i.cursor(), 7);
    }

    #[test]
    fn a_paste_lands_at_the_cursor() {
        let mut i = TextInput::single().with_text("ac");
        i.move_to(1);
        i.paste("b");
        assert_eq!(i.text(), "abc");
        assert_eq!(i.cursor(), 2);
    }

    #[test]
    fn the_character_limit_counts_characters_not_bytes() {
        let mut i = TextInput::single().with_max_chars(4);
        i.paste("字字字字字字");
        assert_eq!(i.text(), "字字字字");
        typed(&mut i, "x");
        assert_eq!(i.text(), "字字字字");
    }

    #[test]
    fn take_hands_over_the_text_and_leaves_an_empty_field() {
        let mut i = TextInput::single().with_text("draft");
        assert_eq!(i.take(), "draft");
        assert!(i.is_empty());
        assert_eq!(i.cursor(), 0);
    }

    // -- rendering ---------------------------------------------------------

    fn render(i: &mut TextInput, w: u16, h: u16) -> (Buffer, Option<(u16, u16)>) {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let cur = i.render(area, &mut buf, Style::default());
        (buf, cur)
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn a_composer_draws_its_wrapped_rows() {
        let mut i = TextInput::multiline().with_text("aaaa bbbb cccc");
        let (buf, cur) = render(&mut i, 5, 3);
        assert_eq!(row(&buf, 0), "aaaa");
        assert_eq!(row(&buf, 1), "bbbb");
        assert_eq!(row(&buf, 2), "cccc");
        assert_eq!(cur, Some((4, 2)), "at the end of the last row");
    }

    #[test]
    fn the_reported_cursor_is_where_the_text_says_it_is() {
        for width in [4u16, 5, 7, 12, 40] {
            let text = "one two three four";
            let mut i = TextInput::multiline().with_text(text);
            for at in 0..=text.len() {
                if !text.is_char_boundary(at) {
                    continue;
                }
                i.move_to(at);
                let (_, cur) = render(&mut i, width, 20);
                let (x, y) = cur.expect("on screen");
                let rows = wrap::wrap(text, width);
                let (row, col) = cursor_cell(text, &rows, at);
                assert_eq!(
                    (x, y),
                    (col.min(width - 1), row),
                    "width {width}, byte {at}"
                );
            }
        }
    }

    #[test]
    fn the_cursor_line_is_scrolled_into_view() {
        let mut i = TextInput::multiline().with_text("1\n2\n3\n4\n5\n6");
        let (buf, cur) = render(&mut i, 10, 3);
        assert_eq!(row(&buf, 0), "4");
        assert_eq!(cur, Some((1, 2)));

        i.move_to(0);
        let (buf, cur) = render(&mut i, 10, 3);
        assert_eq!(row(&buf, 0), "1");
        assert_eq!(cur, Some((0, 0)));
    }

    #[test]
    fn a_one_line_field_scrolls_sideways_to_keep_the_cursor() {
        let mut i = TextInput::single().with_text("abcdefghijklmnop");
        // The cursor sits one column past the last character, so the last
        // column of the field is the caret and only five characters show.
        let (buf, cur) = render(&mut i, 6, 1);
        assert_eq!(row(&buf, 0), "lmnop");
        assert_eq!(cur, Some((5, 0)));

        i.move_to(0);
        let (buf, cur) = render(&mut i, 6, 1);
        assert_eq!(row(&buf, 0), "abcdef");
        assert_eq!(cur, Some((0, 0)));
    }

    #[test]
    fn a_one_line_field_never_draws_half_of_a_wide_character() {
        let mut i = TextInput::single().with_text("字字字字字字");
        let (buf, _) = render(&mut i, 5, 1);
        // Five columns cannot hold three two-cell characters, and the one that
        // does not fit is left out rather than drawn as its left half. The odd
        // columns are the second cell of a wide character.
        assert_eq!(buf[(0, 0)].symbol(), "字");
        assert_eq!(buf[(2, 0)].symbol(), "字");
        assert_eq!(buf[(4, 0)].symbol(), " ");
    }

    // -- generated key sequences -------------------------------------------

    fn any_key() -> impl proptest::strategy::Strategy<Value = KeyEvent> {
        use proptest::prelude::*;
        let codes = prop_oneof![
            proptest::sample::select(vec![
                'a',
                'Z',
                ' ',
                '1',
                '字',
                '\u{301}',
                '\u{1f600}',
                '!',
                'é',
            ])
            .prop_map(KeyCode::Char),
            Just(KeyCode::Backspace),
            Just(KeyCode::Delete),
            Just(KeyCode::Left),
            Just(KeyCode::Right),
            Just(KeyCode::Up),
            Just(KeyCode::Down),
            Just(KeyCode::Home),
            Just(KeyCode::End),
            Just(KeyCode::Enter),
        ];
        let mods = proptest::sample::select(vec![
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
        ]);
        (codes, mods).prop_map(|(code, m)| KeyEvent::new(code, m))
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(300))]

        /// Whatever anybody types, the cursor stays on a character boundary
        /// and inside the text, and nothing panics on the way.
        #[test]
        fn any_sequence_of_keys_leaves_a_usable_field(
            start in "(?s).{0,40}",
            keys in proptest::collection::vec(any_key(), 0..60),
            width in 1u16..12,
        ) {
            let mut i = TextInput::multiline().with_text(start);
            let area = Rect::new(0, 0, width, 4);
            for k in keys {
                i.handle(k);
                proptest::prop_assert!(i.cursor() <= i.text().len());
                proptest::prop_assert!(i.text().is_char_boundary(i.cursor()));
                let mut buf = Buffer::empty(area);
                if let Some((x, y)) = i.render(area, &mut buf, Style::default()) {
                    proptest::prop_assert!(x < area.width && y < area.height);
                }
            }
        }

        #[test]
        fn a_one_line_field_never_ends_up_holding_a_newline(
            text in "(?s).{0,60}",
        ) {
            let mut i = TextInput::single();
            i.paste(&text);
            proptest::prop_assert!(!i.text().contains('\n'));
            proptest::prop_assert!(!i.text().contains('\r'));
        }
    }

    #[test]
    fn an_empty_rect_draws_nothing_and_reports_no_cursor() {
        let mut i = TextInput::single().with_text("text");
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        assert_eq!(i.render(area, &mut buf, Style::default()), None);
    }
}
