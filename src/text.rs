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
}
