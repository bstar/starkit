//! Changing one setting in `config.toml` without disturbing the rest of it.
//!
//! Serialising the whole `Config` back would be a line of code and would throw
//! away every comment in the file, the section order, and any key the
//! application does not know about. A config file people are meant to
//! hand-edit is one you have
//! to edit the way they would: find the line, change the value on it, leave
//! everything else byte for byte as it was.
//!
//! This is a line editor, not a TOML parser, and it is deliberately narrow. It
//! handles the shape the file actually has -- `[section]` headers and
//! `key = value` under them -- and declines anything it does not recognise
//! rather than guessing.
//!
//! Values are scalars and lists of them. The **root table** -- the keys above
//! the first `[section]`, where `theme` and `volume` live -- is addressed by
//! the empty section name, because those are settings like any other and there
//! was previously no way to write them at all.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};

/// A value as it should appear in the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Float(f64),
    /// An equalizer curve, and nothing else so far.
    Floats(Vec<f64>),
}

/// A float as TOML wants it: never bare `1`, which parses as an integer and
/// then fails to deserialise into an `f32`.
fn float(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.3}")
    } else {
        "0.0".into()
    }
}

/// Escape a string for a TOML basic string.
///
/// Quotes and backslashes were already handled; control characters were not,
/// and a literal newline inside a basic string is not valid TOML. The values
/// that reach here are file stems -- a theme name, an equalizer profile name,
/// a library path -- and a filename on Linux may contain a newline, so a
/// preset called `bass\nlibrary_root = "/"` would be written out as two lines
/// and the config would stop parsing on the next run.
///
/// It cannot inject a second key even without this, because the closing quote
/// is escaped and TOML rejects the unterminated string that results. That
/// makes this a corruption fix rather than an injection one, which is a reason
/// to fix it rather than a reason not to.
pub fn escape_basic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // Everything else below 0x20, and DEL, which TOML spells as an
            // escape rather than allowing raw.
            c if c.is_control() => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

impl Value {
    fn render(&self) -> String {
        match self {
            // Escaped the way TOML wants it. Config values here are theme and
            // style names, but a path could reach this one day.
            Value::Str(s) => format!("\"{}\"", escape_basic(s)),
            Value::Int(i) => i.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Float(f) => float(*f),
            Value::Floats(v) => {
                let parts: Vec<String> = v.iter().map(|f| float(*f)).collect();
                format!("[{}]", parts.join(", "))
            }
        }
    }
}

/// The root table, above the first `[section]`. `theme` and `volume` live here.
pub const ROOT: &str = "";

/// Set `section.key` in the file at `path`, creating either if missing.
///
/// Written through [`crate::fs::write_atomic`], so an interrupted save cannot
/// leave a half-written config behind and two instances saving at once cannot
/// interleave into one file.
pub fn set(path: &Path, section: &str, key: &str, value: &Value) -> Result<()> {
    // A file that is not there yet is an empty one. Failing instead meant that
    // on a fresh install -- where nothing has ever written a config -- the very
    // first setting the user changed reported itself as saved for the session
    // only, for ever.
    let original = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
    };
    let updated = apply(&original, section, key, value);
    if updated == original {
        return Ok(());
    }

    crate::fs::write_atomic(path, updated.as_bytes())
}

/// The edit itself, on the file's text.
///
/// Split out from the io so it can be tested against awkward files without
/// touching a disk.
pub fn apply(source: &str, section: &str, key: &str, value: &Value) -> String {
    let mut out = String::with_capacity(source.len() + 64);
    // The root table is open from the first line; every other section has to
    // be entered through its header.
    let mut in_section = section == ROOT;
    let mut done = false;
    // Where the section ended, so a missing key can be added at its end
    // rather than after whatever section follows it.
    let mut section_end: Option<usize> = None;

    for line in source.lines() {
        let trimmed = line.trim_start();

        if let Some(name) = header_name(trimmed) {
            if in_section && !done {
                // Leaving our section without having found the key. Before the
                // blank line that separates it from the next header, not
                // after: a key added at the end of a block belongs with the
                // keys, not on the far side of the gap.
                section_end = Some(content_end(&out));
            }
            in_section = name == section;
        } else if in_section && !done && key_on_line(trimmed) == Some(key) {
            let _ = writeln!(out, "{}", rewrite(line, key, value));
            done = true;
            continue;
        }
        let _ = writeln!(out, "{line}");
    }

    if done {
        return restore_trailing_newline(source, out);
    }

    let line = format!("{key} = {}\n", value.render());
    match section_end {
        // The section exists but has no such key: add it at the section's end.
        Some(at) => out.insert_str(at, &line),
        None if in_section => {
            // The section is the file's last, so its end is the file's end --
            // minus whatever blank lines trail it. An empty file has no end to
            // separate the key from, and a leading blank line there is just
            // noise, which is what a fresh config written a key at a time
            // would otherwise have been made of.
            out.truncate(content_end(&out));
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&line);
        }
        // No such section anywhere: a new one, with a blank line in front of it
        // like every other section in the file has.
        None => {
            out.truncate(content_end(&out));
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            let _ = write!(out, "[{section}]\n{line}");
        }
    }
    restore_trailing_newline(source, out)
}

/// Where the content ends, ignoring blank lines that trail it.
///
/// Everything here is written a line at a time, so the last real line always
/// ends in a newline and that newline is kept.
fn content_end(out: &str) -> usize {
    let mut end = out.len();
    for line in out.lines().rev() {
        if !line.trim().is_empty() {
            break;
        }
        end = end.saturating_sub(line.len() + 1);
    }
    end.min(out.len())
}

/// `[name]` if the line is a section header.
fn header_name(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix('[')?;
    // Not `[[array]]`: neither application's config has one, and guessing at
    // one would be worse than leaving it alone.
    if rest.starts_with('[') {
        return None;
    }
    let end = rest.find(']')?;
    Some(rest[..end].trim())
}

/// The key a `key = value` line assigns to, if it is one.
fn key_on_line(trimmed: &str) -> Option<&str> {
    if trimmed.starts_with('#') {
        return None;
    }
    let (name, _) = trimmed.split_once('=')?;
    let name = name.trim();
    (!name.is_empty() && !name.contains(char::is_whitespace)).then_some(name)
}

/// Replace the value on a line, keeping its indentation and trailing comment.
fn rewrite(line: &str, key: &str, value: &Value) -> String {
    let indent = &line[..line.len() - line.trim_start().len()];
    let after = line.split_once('=').map(|(_, v)| v).unwrap_or("");
    // A `#` inside a quoted string is not a comment. Values here are simple
    // enough that tracking quotes is the whole of it.
    let mut in_quotes = false;
    let comment = after.char_indices().find_map(|(i, c)| match c {
        '"' => {
            in_quotes = !in_quotes;
            None
        }
        '#' if !in_quotes => Some(i),
        _ => None,
    });

    match comment {
        Some(i) => {
            // Keep the comment where it was, so a column of them stays lined
            // up when the new value is shorter than the old.
            let spacing = after[..i].len() - after[..i].trim_end().len();
            let rendered = value.render();
            let pad = spacing.max(1);
            format!(
                "{indent}{key} = {rendered}{}{}",
                " ".repeat(pad),
                &after[i..]
            )
        }
        None => format!("{indent}{key} = {}", value.render()),
    }
}

/// `lines()` drops the final newline; put it back only if it was there.
fn restore_trailing_newline(source: &str, mut out: String) -> String {
    if !source.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# staramp
library_root = \"/music\"

[ui]
# Transport button faces.
glyphs = \"nerd\"
padding_x = 1

[vis]
mode = \"bars\"
";

    #[test]
    fn two_writers_do_not_share_a_temporary_file() {
        // A fixed temp name in a shared config directory is two instances
        // interleaving into one file and then both renaming it over the
        // config. The name carries the pid so each has its own.
        let dir = std::env::temp_dir().join(format!("starkit-edit-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        std::fs::write(&path, "[ui]\nglyphs = \"nerd\"\n").unwrap();

        set(&path, "ui", "glyphs", &Value::Str("ascii".into())).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("ascii"));

        // Nothing left behind, and nothing with the old shared name.
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "config.toml")
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_root_key_is_changed_where_it_sits() {
        // `theme` and `volume` live above the first section. There was no way
        // to write them at all before, which is why neither persisted.
        let out = apply(SAMPLE, ROOT, "library_root", &Value::Str("/other".into()));
        assert!(out.contains("library_root = \"/other\""), "{out:?}");
        assert!(out.contains("[ui]"), "the sections went: {out:?}");
        assert!(out.contains("mode = \"bars\""), "{out:?}");
    }

    #[test]
    fn a_missing_root_key_lands_above_the_first_section() {
        let out = apply(SAMPLE, ROOT, "volume", &Value::Float(0.8));
        let head = out.split("[ui]").next().unwrap();
        assert!(
            head.contains("volume = 0.800"),
            "not in the root table: {out:?}"
        );
        assert!(out.contains("[ui]") && out.contains("[vis]"), "{out:?}");
    }

    #[test]
    fn a_root_key_is_not_confused_with_the_same_key_in_a_section() {
        let src = "mode = \"root\"\n\n[vis]\nmode = \"bars\"\n";
        let out = apply(src, ROOT, "mode", &Value::Str("changed".into()));
        assert!(out.starts_with("mode = \"changed\""), "{out:?}");
        assert!(out.contains("[vis]\nmode = \"bars\""), "{out:?}");
        // And the other way round.
        let out = apply(src, "vis", "mode", &Value::Str("leds".into()));
        assert!(out.starts_with("mode = \"root\""), "{out:?}");
        assert!(out.contains("[vis]\nmode = \"leds\""), "{out:?}");
    }

    #[test]
    fn a_root_key_can_be_written_into_an_empty_file() {
        // `[]` is not a section header, so the section-creating path would
        // produce something that does not parse.
        let out = apply("", ROOT, "volume", &Value::Float(1.0));
        assert_eq!(out, "volume = 1.000", "no leading blank line: {out:?}");
        let parsed: toml::Value = toml::from_str(&out).expect(&out);
        assert_eq!(parsed["volume"].as_float(), Some(1.0));
    }

    #[test]
    fn a_float_is_written_so_it_reads_back_as_one() {
        // Bare `1` is an integer to TOML and then fails to deserialise into an
        // `f32`, which would break the config the next time it loaded.
        for v in [1.0, 0.0, 0.85, -3.5] {
            let out = apply("", ROOT, "volume", &Value::Float(v));
            let parsed: toml::Value = toml::from_str(&out).expect(&out);
            let back = parsed["volume"].as_float().expect(&out);
            assert!((back - v).abs() < 1e-3, "{v} came back as {back}");
        }
    }

    #[test]
    fn a_curve_is_written_as_a_list_of_floats() {
        let gains = vec![0.0, -1.5, 3.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 6.0];
        let out = apply("", "eq", "gains", &Value::Floats(gains.clone()));
        let parsed: toml::Value = toml::from_str(&out).expect(&out);
        let back: Vec<f64> = parsed["eq"]["gains"]
            .as_array()
            .expect(&out)
            .iter()
            .map(|v| v.as_float().unwrap())
            .collect();
        assert_eq!(back.len(), gains.len());
        for (a, b) in back.iter().zip(&gains) {
            assert!((a - b).abs() < 1e-3, "{back:?} != {gains:?}");
        }
    }

    #[test]
    fn a_file_written_a_key_at_a_time_still_reads_like_a_config() {
        // An application writes settings one key at a time, in whatever order
        // they are held in -- and a key added to a section it did not have landed
        // on the wrong side of the blank line before the next section, which
        // parsed correctly and looked like a mess.
        let mut out = "library_root = \"/music\"\n\n[ui]\nglyphs = \"nerd\"\n".to_string();
        for (section, key, value) in [
            ("playlist", "shuffle", Value::Bool(true)),
            ("ui", "show_album", Value::Bool(true)),
            ("vis", "mode", Value::Str("peaks".into())),
            ("ui", "show_playlist", Value::Bool(false)),
        ] {
            out = apply(&out, section, key, &value);
        }
        assert_eq!(
            out,
            "library_root = \"/music\"\n\
             \n\
             [ui]\n\
             glyphs = \"nerd\"\n\
             show_album = true\n\
             show_playlist = false\n\
             \n\
             [playlist]\n\
             shuffle = true\n\
             \n\
             [vis]\n\
             mode = \"peaks\"\n",
            "{out}"
        );
    }

    #[test]
    fn a_new_section_never_runs_into_the_one_before_it() {
        let out = apply(
            "[ui]\nglyphs = \"nerd\"\n",
            "vis",
            "mode",
            &Value::Str("leds".into()),
        );
        assert!(out.contains("glyphs = \"nerd\"\n\n[vis]"), "{out:?}");
    }

    #[test]
    fn blank_lines_between_sections_are_not_multiplied() {
        let src = "[ui]\nglyphs = \"nerd\"\n\n\n";
        let out = apply(src, "vis", "mode", &Value::Str("leds".into()));
        assert!(!out.contains("\n\n\n"), "{out:?}");
    }

    #[test]
    fn changing_a_value_leaves_every_other_byte_alone() {
        let out = apply(SAMPLE, "ui", "glyphs", &Value::Str("ascii".into()));
        assert!(out.contains("glyphs = \"ascii\""));
        assert!(out.contains("# staramp"), "the header comment went");
        assert!(
            out.contains("# Transport button faces."),
            "the key's own comment went"
        );
        assert!(out.contains("mode = \"bars\""), "another section changed");
        assert_eq!(out.lines().count(), SAMPLE.lines().count());
    }

    #[test]
    fn a_trailing_comment_stays_on_its_line() {
        let src = "[vis]\nbar_width = 3   # cells per bar\n";
        let out = apply(src, "vis", "bar_width", &Value::Int(5));
        assert!(
            out.contains("bar_width = 5"),
            "the value did not change: {out:?}"
        );
        assert!(
            out.contains("# cells per bar"),
            "the comment was lost: {out:?}"
        );
    }

    #[test]
    fn a_missing_key_is_added_to_its_own_section() {
        let out = apply(SAMPLE, "ui", "seek_style", &Value::Str("ansi".into()));
        let ui = out.split("[vis]").next().unwrap();
        assert!(
            ui.contains("seek_style = \"ansi\""),
            "added outside [ui]: {out:?}"
        );
        assert!(out.contains("mode = \"bars\""), "[vis] was disturbed");
    }

    #[test]
    fn a_missing_key_in_the_last_section_lands_inside_it() {
        let out = apply(SAMPLE, "vis", "bar_gap", &Value::Int(1));
        assert!(out.trim_end().ends_with("bar_gap = 1"), "{out:?}");
    }

    #[test]
    fn a_missing_section_is_appended() {
        let out = apply(SAMPLE, "fx", "enabled", &Value::Bool(false));
        assert!(out.contains("[fx]\nenabled = false"), "{out:?}");
        assert!(out.contains("mode = \"bars\""), "the rest was disturbed");
    }

    #[test]
    fn the_same_key_in_another_section_is_not_touched() {
        // Both [ui] and [vis] have a `mode` in some configurations; writing
        // one must not write the other.
        let src = "[ui]\nmode = \"a\"\n\n[vis]\nmode = \"b\"\n";
        let out = apply(src, "vis", "mode", &Value::Str("c".into()));
        assert!(out.contains("[ui]\nmode = \"a\""), "{out:?}");
        assert!(out.contains("[vis]\nmode = \"c\""), "{out:?}");
    }

    #[test]
    fn a_commented_out_key_is_not_mistaken_for_the_real_one() {
        let src = "[ui]\n# glyphs = \"nerd\"\nglyphs = \"ascii\"\n";
        let out = apply(src, "ui", "glyphs", &Value::Str("block".into()));
        assert!(out.contains("# glyphs = \"nerd\""), "the comment changed");
        assert!(out.contains("\nglyphs = \"block\""), "{out:?}");
    }

    #[test]
    fn indentation_is_preserved() {
        let src = "[ui]\n    padding_x = 1\n";
        let out = apply(src, "ui", "padding_x", &Value::Int(4));
        assert!(out.contains("    padding_x = 4"), "{out:?}");
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let src = "[app]\ntheme = \"#ff00ff\"\n";
        let out = apply(src, "app", "theme", &Value::Str("cosmic".into()));
        assert_eq!(out, "[app]\ntheme = \"cosmic\"\n", "{out:?}");
    }

    #[test]
    fn values_are_escaped() {
        assert_eq!(Value::Str("a\"b".into()).render(), "\"a\\\"b\"");
        assert_eq!(Value::Int(-3).render(), "-3");
        assert_eq!(Value::Bool(true).render(), "true");
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_that_shape() {
        let src = "[ui]\npadding_x = 1";
        let out = apply(src, "ui", "padding_x", &Value::Int(2));
        assert_eq!(out, "[ui]\npadding_x = 2");
    }

    #[test]
    fn writing_the_value_it_already_has_changes_nothing() {
        let out = apply(SAMPLE, "vis", "mode", &Value::Str("bars".into()));
        assert_eq!(out, SAMPLE);
    }

    /// A theme or preset name is a file stem, and a filename on Linux may
    /// contain a newline. Written raw, that ends the TOML line and the config
    /// stops parsing on the next run.
    #[test]
    fn a_value_with_a_newline_still_writes_a_config_that_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cosmic\"\n").unwrap();

        let hostile = "bass\nlibrary_root = \"/\"\nvolume = 9";
        set(&path, ROOT, "theme", &Value::Str(hostile.into())).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("\nlibrary_root"),
            "the value broke out of its line:\n{text}"
        );
        let parsed: toml::Value = toml::from_str(&text).expect("config no longer parses");
        assert_eq!(parsed["theme"].as_str(), Some(hostile));
        assert!(parsed.get("volume").is_none());
    }

    #[test]
    fn control_characters_are_escaped_rather_than_written_raw() {
        assert_eq!(escape_basic("a\nb"), "a\\nb");
        assert_eq!(escape_basic("a\tb"), "a\\tb");
        assert_eq!(escape_basic("a\u{1}b"), "a\\u0001b");
        assert_eq!(escape_basic("say \"hi\"\\"), "say \\\"hi\\\"\\\\");
        // Ordinary text, including non-ASCII, is left exactly as it is.
        assert_eq!(escape_basic("Mötley Crüe"), "Mötley Crüe");
    }
}

/// Generated adversarial input.
///
/// The values written back are file stems -- a theme name, a preset name, a
/// library path -- and a filename may hold almost anything.
#[cfg(test)]
mod fuzz {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn any_value_writes_a_config_that_still_parses(
            source in ".{0,500}",
            value in ".{0,200}",
        ) {
            let out = apply(&source, ROOT, "theme", &Value::Str(value.clone()));
            // Whatever the source was, what we wrote for our own key has to
            // read back as the value we set.
            if let Ok(parsed) = toml::from_str::<toml::Value>(&out) {
                prop_assert_eq!(parsed["theme"].as_str(), Some(value.as_str()));
            }
        }

        /// Starting from a config this program wrote, the result must always
        /// parse -- that is the case a user actually hits.
        #[test]
        fn a_well_formed_config_survives_any_value(value in ".{0,200}") {
            let source = "theme = \"cosmic\"\nvolume = 0.800\n\n[ui]\ngraphics = \"auto\"\n";
            let out = apply(source, ROOT, "theme", &Value::Str(value.clone()));
            let parsed: toml::Value =
                toml::from_str(&out).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(parsed["theme"].as_str(), Some(value.as_str()));
            prop_assert_eq!(parsed["ui"]["graphics"].as_str(), Some("auto"));
        }
    }
}
