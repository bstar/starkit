//! A three-row seven-segment font, for a clock or an elapsed time.
//!
//! Winamp's `NUMBERS.BMP` is the single most recognisable part of its main
//! window after the analyzer, and a plain `2:14` does not read as Winamp at
//! all. Box-drawing characters rather than a bitmap, so it needs no font the
//! terminal might not have.

/// Rows of glyph art for a digit, `0`-`9`, plus `:` and a blank.
pub fn glyph(c: char) -> [&'static str; 3] {
    match c {
        '0' => ["┏━┓", "┃ ┃", "┗━┛"],
        '1' => ["╺┓ ", " ┃ ", "╺┻╸"],
        '2' => ["╺━┓", "┏━┛", "┗━╸"],
        '3' => ["╺━┓", " ━┫", "╺━┛"],
        '4' => ["╻ ╻", "┗━┫", "  ╹"],
        '5' => ["┏━╸", "┗━┓", "╺━┛"],
        '6' => ["┏━╸", "┣━┓", "┗━┛"],
        '7' => ["╺━┓", "  ┃", "  ╹"],
        '8' => ["┏━┓", "┣━┫", "┗━┛"],
        '9' => ["┏━┓", "┗━┫", "╺━┛"],
        ':' => [" ▪ ", "   ", " ▪ "],
        '-' => ["   ", "╺━╸", "   "],
        _ => ["   ", "   ", "   "],
    }
}

/// Render `text` as three rows of large digits.
pub fn render(text: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        for r in 0..3 {
            if i > 0 {
                rows[r].push(' ');
            }
            rows[r].push_str(g[r]);
        }
    }
    rows
}

/// `m:ss` for a duration in seconds, or `-:--` when unknown.
pub fn clock(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "-:--".into();
    }
    let total = secs as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// `mm:ss` zero-padded, for the compact readouts.
pub fn clock_padded(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".into();
    }
    let total = secs as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_digit_is_three_cells_wide_on_every_row() {
        for c in "0123456789:-".chars() {
            let g = glyph(c);
            for row in g {
                assert_eq!(row.chars().count(), 3, "glyph {c:?} row {row:?}");
            }
        }
    }

    #[test]
    fn rendering_keeps_all_three_rows_the_same_width() {
        let rows = render("12:34");
        let w: Vec<usize> = rows.iter().map(|r| r.chars().count()).collect();
        assert_eq!(w[0], w[1]);
        assert_eq!(w[1], w[2]);
        // 5 glyphs of 3 cells, plus 4 single-space gaps.
        assert_eq!(w[0], 5 * 3 + 4);
    }

    #[test]
    fn clock_formats_and_handles_the_unknown_case() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(134.0), "2:14");
        assert_eq!(clock(3599.0), "59:59");
        assert_eq!(clock(f64::NAN), "-:--");
        assert_eq!(clock(-1.0), "-:--");
        assert_eq!(clock_padded(134.0), "02:14");
    }
}
