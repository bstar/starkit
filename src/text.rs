//! Fitting text into a fixed number of columns.
//!
//! Both of these measure display width rather than counting characters. A
//! title is whatever somebody typed, and half the interesting ones are CJK or
//! carry an emoji: `chars().take(n)` overflows the panel on one and cuts a
//! character in half on the other.

/// Scroll a string that does not fit, looping with a separator.
pub fn marquee(s: &str, width: usize, offset: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if width == 0 {
        return String::new();
    }
    if s.width() <= width {
        return s.to_string();
    }
    let padded = format!("{s}   ***   ");
    let chars: Vec<char> = padded.chars().collect();
    let start = offset % chars.len();
    chars.iter().cycle().skip(start).take(width).collect()
}

/// Cut and pad a row to exactly `width` display columns.
///
/// By display width, never by character count. Titles, channel names and the
/// people in a chat all routinely contain emoji, and an emoji is two columns;
/// a row measured in characters is a row one cell wider than the panel it is
/// in, which writes over the border and leaves it there until something else
/// redraws it. That is the artefact this function exists to prevent, and it
/// is why no panel formats a row with `{:width$}`.
pub fn fit(text: &str, width: u16) -> String {
    let mut out = String::with_capacity(usize::from(width) + 4);
    let mut used = 0u16;
    for (_, cluster) in crate::wrap::clusters(text) {
        let w = crate::wrap::width_of(cluster);
        if used + w > width {
            break;
        }
        out.push_str(cluster);
        used += w;
    }
    // A double-width cluster at the edge leaves one column over; a space is
    // what fills it, because a half-drawn emoji is not a thing a terminal can
    // show.
    for _ in used..width {
        out.push(' ');
    }
    out
}

/// Truncate to a display width, with an ellipsis.
pub fn truncate(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if width == 0 {
        return String::new();
    }
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_titles_are_left_alone() {
        assert_eq!(marquee("Short", 20, 0), "Short");
        assert_eq!(marquee("Short", 20, 7), "Short", "offset is irrelevant");
    }

    #[test]
    fn long_titles_scroll_and_wrap_around() {
        let s = "A Very Long Track Title That Does Not Fit At All";
        let a = marquee(s, 10, 0);
        let b = marquee(s, 10, 1);
        assert_eq!(a.chars().count(), 10);
        assert_eq!(b.chars().count(), 10);
        assert_ne!(a, b, "it should actually move");
    }

    #[test]
    fn truncate_respects_display_width_not_byte_length() {
        assert_eq!(truncate("hello", 10), "hello");
        let t = truncate("hello world", 8);
        assert!(t.ends_with('…'));
        use unicode_width::UnicodeWidthStr;
        assert!(t.width() <= 8);
    }

    #[test]
    fn truncate_handles_wide_characters_without_overflowing() {
        // A CJK title is two cells per character; counting chars would overrun
        // the column and corrupt the row.
        use unicode_width::UnicodeWidthStr;
        let s = "君の名は。星を追う子ども";
        for w in [4, 7, 10, 13] {
            assert!(truncate(s, w).width() <= w, "width {w} overflowed");
        }
    }

    #[test]
    fn zero_width_produces_nothing_rather_than_panicking() {
        assert_eq!(truncate("anything", 0), "");
        assert_eq!(marquee("anything", 0, 3), "");
    }

    #[test]
    fn fit_pads_short_text_with_spaces() {
        assert_eq!(fit("hi", 5), "hi   ");
        use unicode_width::UnicodeWidthStr;
        assert_eq!(fit("hi", 5).width(), 5);
    }

    #[test]
    fn fit_cuts_by_display_width() {
        use unicode_width::UnicodeWidthStr;
        let s = "君の名は。星を追う子ども";
        for w in [4u16, 7, 10, 13] {
            let f = fit(s, w);
            assert_eq!(f.width(), w as usize, "width {w}: {f:?}");
        }
    }

    #[test]
    fn fit_drops_an_emoji_that_would_straddle_the_edge() {
        // "a🎧" is 1 + 2 = 3 columns; asking for 2 cannot show the emoji
        // half-drawn, so it is dropped and the column is padded instead.
        let f = fit("a\u{1f3a7}", 2);
        assert_eq!(f, "a ");
        use unicode_width::UnicodeWidthStr;
        assert_eq!(f.width(), 2);
    }
}
