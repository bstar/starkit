//! Importing classic Winamp skins.
//!
//! A `.wsz` is a plain ZIP of BMPs plus two text files, and the two text files
//! carry almost everything that matters:
//!
//! - `VISCOLOR.TXT` — 24 `r,g,b` lines. Indices 2..17 are the analyzer ramp
//!   (top to bottom), 18 is the peak cap, 1 is the dot grid, 0 the background,
//!   and 19..23 the oscilloscope shades. This is the file that makes an
//!   imported skin's analyzer look right rather than approximately right.
//! - `PLEDIT.TXT` — an INI-ish `[Text]` section with `Normal`, `Current`,
//!   `NormalBG` and `SelectedBG`.
//!
//! Anything a skin omits falls back to the built-in Winamp 2.91 values and then
//! to the ordinary derivation chain, so a partial skin still produces a
//! complete theme.

use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use super::color::Rgb;

#[derive(Debug, Clone, Default)]
pub struct SkinColors {
    /// 24 entries when the skin supplied VISCOLOR.TXT.
    pub viscolor: Vec<Rgb>,
    pub pledit_normal: Option<Rgb>,
    pub pledit_current: Option<Rgb>,
    pub pledit_normal_bg: Option<Rgb>,
    pub pledit_selected_bg: Option<Rgb>,
    /// Sampled from TEXT.BMP, for the marquee.
    pub text_fg: Option<Rgb>,
    pub text_bg: Option<Rgb>,
    pub warnings: Vec<String>,
}

/// The most VISCOLOR.TXT or PLEDIT.TXT may expand to. Both are a couple of
/// dozen short lines in every real skin.
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// The most TEXT.BMP may expand to. The real ones are a few kilobytes.
const MAX_BITMAP_BYTES: u64 = 8 * 1024 * 1024;

/// Winamp's font sheet is 155x18. Anything remotely this large is not one.
const MAX_BITMAP_DIMENSION: u32 = 4096;

/// Read a `.wsz` (or `.zip`) and extract everything we can colour from.
pub fn read(path: &Path) -> Result<SkinColors> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a zip archive", path.display()))?;

    // Winamp skins are wildly inconsistent about case and sometimes nest
    // everything one directory deep.
    let mut index: Vec<(usize, String)> = Vec::new();
    for i in 0..zip.len() {
        if let Ok(f) = zip.by_index(i) {
            let name = f
                .name()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            index.push((i, name));
        }
    }

    if index.iter().any(|(_, n)| n == "skin.xml") {
        return Err(anyhow!(
            "{}: this is a Winamp 5 'modern' skin (.wal); only classic .wsz skins are supported",
            path.display()
        ));
    }

    let mut out = SkinColors::default();

    // A skin is a zip somebody downloaded, and deflate does not have to be
    // honest about what it expands to: a ten-megabyte archive whose
    // VISCOLOR.TXT inflates a thousandfold is ten gigabytes of resident memory
    // before anything looks at it. The declared size is checked first, and the
    // read is capped anyway in case the header is lying, which nothing stops a
    // hostile archive from doing.
    //
    // A refusal is a warning rather than a silent fallback, because "your
    // colours look wrong" with nothing said is the one outcome that wastes
    // somebody's afternoon.
    let read_entry = |zip: &mut zip::ZipArchive<std::fs::File>,
                      want: &str,
                      limit: u64,
                      warnings: &mut Vec<String>|
     -> Option<Vec<u8>> {
        let idx = index.iter().find(|(_, n)| n == want).map(|(i, _)| *i)?;
        let f = zip.by_index(idx).ok()?;
        let declared = f.size();
        if declared > limit {
            warnings.push(format!(
                "{want} says it holds {declared} bytes, over the {limit}-byte limit; ignored"
            ));
            return None;
        }
        let mut buf = Vec::new();
        if f.take(limit + 1).read_to_end(&mut buf).is_err() {
            warnings.push(format!("{want} could not be read; ignored"));
            return None;
        }
        if buf.len() as u64 > limit {
            warnings.push(format!(
                "{want} expands past the {limit}-byte limit; ignored"
            ));
            return None;
        }
        Some(buf)
    };

    if let Some(bytes) = read_entry(&mut zip, "viscolor.txt", MAX_TEXT_BYTES, &mut out.warnings) {
        let (colors, warn) = parse_viscolor(&String::from_utf8_lossy(&bytes));
        out.viscolor = colors;
        out.warnings.extend(warn);
    } else {
        out.warnings
            .push("no VISCOLOR.TXT; analyzer colours will be derived".into());
    }

    if let Some(bytes) = read_entry(&mut zip, "pledit.txt", MAX_TEXT_BYTES, &mut out.warnings) {
        let p = parse_pledit(&String::from_utf8_lossy(&bytes));
        out.pledit_normal = p.0;
        out.pledit_current = p.1;
        out.pledit_normal_bg = p.2;
        out.pledit_selected_bg = p.3;
    } else {
        out.warnings
            .push("no PLEDIT.TXT; playlist colours will be derived".into());
    }

    // The bitmap font sheet is two-tone, so a histogram identifies it exactly:
    // most frequent colour is the background, second most is the glyph.
    if let Some(bytes) = read_entry(&mut zip, "text.bmp", MAX_BITMAP_BYTES, &mut out.warnings) {
        match crate::graphics::decode_limited(&bytes, MAX_BITMAP_DIMENSION) {
            Ok(img) => {
                let (bg, fg) = two_tone(&img.to_rgb8());
                out.text_bg = bg;
                out.text_fg = fg;
            }
            Err(e) => out
                .warnings
                .push(format!("TEXT.BMP could not be read: {e}")),
        }
    }

    Ok(out)
}

/// `VISCOLOR.TXT`: 24 lines of `r,g,b`, often with `// comment` trailing.
pub fn parse_viscolor(text: &str) -> (Vec<Rgb>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();

    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 3 {
            continue;
        }
        match (
            parts[0].parse::<u8>(),
            parts[1].parse::<u8>(),
            parts[2].parse::<u8>(),
        ) {
            (Ok(r), Ok(g), Ok(b)) => out.push(Rgb::new(r, g, b)),
            _ => continue,
        }
        if out.len() == 24 {
            break;
        }
    }

    if out.len() < 24 {
        warnings.push(format!(
            "VISCOLOR.TXT has {} of 24 entries; the rest fall back to the base skin",
            out.len()
        ));
    }
    (out, warnings)
}

/// `PLEDIT.TXT`: returns `(normal, current, normal_bg, selected_bg)`.
pub fn parse_pledit(text: &str) -> (Option<Rgb>, Option<Rgb>, Option<Rgb>, Option<Rgb>) {
    let mut normal = None;
    let mut current = None;
    let mut normal_bg = None;
    let mut selected_bg = None;

    for line in text.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = Rgb::parse_hex(v.trim()).ok();
        match key.as_str() {
            "normal" => normal = val,
            "current" => current = val,
            "normalbg" => normal_bg = val,
            "selectedbg" => selected_bg = val,
            _ => {}
        }
    }
    (normal, current, normal_bg, selected_bg)
}

/// The two most frequent colours in an image, most frequent first.
fn two_tone(img: &image::RgbImage) -> (Option<Rgb>, Option<Rgb>) {
    use std::collections::HashMap;
    let mut counts: HashMap<(u8, u8, u8), usize> = HashMap::new();
    for p in img.pixels() {
        *counts.entry((p[0], p[1], p[2])).or_insert(0) += 1;
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let get = |i: usize| v.get(i).map(|((r, g, b), _)| Rgb::new(*r, *g, *b));
    (get(0), get(1))
}

/// Render an imported skin as a theme file.
pub fn to_theme_toml(skin: &SkinColors, name: &str, source: &str) -> String {
    // Both of these are file stems from a skin somebody downloaded, and a
    // filename may legally contain a quote or a newline. Written raw they end
    // the TOML string early and the theme stops parsing -- or, with a newline,
    // become extra lines in the file. The same escaping the config editor uses.
    let name = crate::config::edit::escape_basic(name);
    let source = crate::config::edit::escape_basic(source);
    let id = name.to_lowercase().replace(' ', "-");
    let mut s = String::new();
    s.push_str(&format!(
        "# Imported from {source}.\n\
         # Analyzer colours are VISCOLOR.TXT verbatim; text colours are PLEDIT.TXT.\n\n\
         [meta]\nname = \"{name}\"\nid = \"{id}\"\nvariant = \"dark\"\nsource = \"{source}\"\n\n"
    ));

    let bg = skin.pledit_normal_bg.unwrap_or_else(|| Rgb::new(0, 0, 0));
    let fg = skin.pledit_normal.unwrap_or_else(|| Rgb::new(0, 255, 0));
    let accent = skin.text_fg.unwrap_or(fg);

    s.push_str("[app]\n");
    s.push_str(&format!("bg     = \"{}\"\n", bg.to_hex()));
    s.push_str(&format!("fg     = \"{}\"\n", fg.to_hex()));
    s.push_str(&format!("accent = \"{}\"\n", accent.to_hex()));
    if skin.viscolor.len() >= 24 {
        // ok/warn/error come from the ends and middle of the analyzer ramp, so
        // the rest of the UI matches the skin rather than the defaults.
        s.push_str(&format!("ok     = \"{}\"\n", skin.viscolor[17].to_hex()));
        s.push_str(&format!("warn   = \"{}\"\n", skin.viscolor[9].to_hex()));
        s.push_str(&format!("error  = \"{}\"\n", skin.viscolor[2].to_hex()));
    }
    s.push('\n');

    s.push_str("[row]\n");
    s.push_str(&format!("fg = \"{}\"\n", fg.to_hex()));
    if let Some(c) = skin.pledit_current {
        s.push_str(&format!("playing_fg = \"{}\"\n", c.to_hex()));
    }
    if let Some(c) = skin.pledit_selected_bg {
        s.push_str(&format!("selected_bg = \"{}\"\n", c.to_hex()));
    }
    s.push('\n');

    if let Some(c) = skin.text_fg {
        s.push_str(&format!("[marquee]\nfg = \"{}\"\n\n", c.to_hex()));
    }

    if skin.viscolor.len() >= 24 {
        s.push_str("[vis]\n");
        s.push_str(&format!("bg      = \"{}\"\n", skin.viscolor[0].to_hex()));
        s.push_str(&format!("grid_fg = \"{}\"\n", skin.viscolor[1].to_hex()));
        s.push_str(&format!("peak_fg = \"{}\"\n", skin.viscolor[18].to_hex()));
        s.push_str("grid    = \"dots\"\n");
        // VISCOLOR 2..17 runs loud-to-quiet; our ramp runs quiet-to-loud.
        s.push_str("ramp = [\n");
        for chunk in (2..=17).rev().collect::<Vec<_>>().chunks(4) {
            s.push_str("  ");
            for i in chunk {
                s.push_str(&format!("\"{}\", ", skin.viscolor[*i].to_hex()));
            }
            s.push('\n');
        }
        s.push_str("]\n");
        s.push_str("osc = [");
        for i in 19..=23 {
            s.push_str(&format!("\"{}\", ", skin.viscolor[i].to_hex()));
        }
        s.push_str("]\n");
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real Winamp 2.91 base skin VISCOLOR.TXT, abbreviated to its shape.
    const VISCOLOR: &str = "\
0,0,0,           // background
24,33,41,        // dot grid
239,49,16,       // 2  top of the analyzer
206,41,16,
214,90,0,
214,102,0,
214,115,0,
198,123,8,
222,165,24,
214,181,33,     // 10
189,222,41,
148,222,33,
41,206,16,
50,190,16,
57,181,16,
49,156,8,
41,148,0,
33,140,0,       // 17 bottom of the analyzer
255,255,255,    // 18 peak cap
214,214,222,    // 19 oscilloscope
181,189,189,
160,170,175,
148,156,165,
150,150,150,
";

    #[test]
    fn viscolor_parses_all_twenty_four_entries() {
        let (c, w) = parse_viscolor(VISCOLOR);
        assert_eq!(c.len(), 24, "warnings: {w:?}");
        assert!(w.is_empty());
        assert_eq!(c[0], Rgb::new(0, 0, 0));
        assert_eq!(c[1], Rgb::new(24, 33, 41));
        assert_eq!(c[2], Rgb::new(239, 49, 16), "top of the analyzer");
        assert_eq!(c[18], Rgb::new(255, 255, 255), "peak cap");
    }

    #[test]
    fn viscolor_ignores_comments_and_blank_lines() {
        let (c, _) = parse_viscolor("// header\n\n1,2,3\n\n4,5,6 // trailing\n");
        assert_eq!(c.len(), 2);
        assert_eq!(c[1], Rgb::new(4, 5, 6));
    }

    #[test]
    fn a_short_viscolor_warns_rather_than_failing() {
        let (c, w) = parse_viscolor("1,2,3\n");
        assert_eq!(c.len(), 1);
        assert!(!w.is_empty(), "a truncated file should say so");
    }

    #[test]
    fn pledit_reads_the_four_colours_case_insensitively() {
        let text =
            "[Text]\nNormal=#00FF00\nCurrent=#FFFFFF\nNormalBG=#000000\nSELECTEDBG=#0000C6\n";
        let (n, c, nbg, sbg) = parse_pledit(text);
        assert_eq!(n, Some(Rgb::new(0, 255, 0)));
        assert_eq!(c, Some(Rgb::new(255, 255, 255)));
        assert_eq!(nbg, Some(Rgb::new(0, 0, 0)));
        assert_eq!(sbg, Some(Rgb::new(0, 0, 0xC6)));
    }

    #[test]
    fn the_generated_theme_parses_and_keeps_the_ramp_verbatim() {
        let (viscolor, _) = parse_viscolor(VISCOLOR);
        let skin = SkinColors {
            viscolor,
            pledit_normal: Some(Rgb::new(0, 255, 0)),
            pledit_current: Some(Rgb::new(255, 255, 255)),
            pledit_normal_bg: Some(Rgb::new(0, 0, 0)),
            pledit_selected_bg: Some(Rgb::new(0, 0, 0xC6)),
            ..Default::default()
        };
        let toml = to_theme_toml(&skin, "Base", "base.wsz");
        let file = super::super::schema::ThemeFile::parse(&toml)
            .unwrap_or_else(|e| panic!("generated theme did not parse: {e}\n{toml}"));
        let t = super::super::resolve::Theme::resolve(&file);

        assert_eq!(t.row_selected_bg, Rgb::new(0, 0, 0xC6));
        assert_eq!(t.row_playing_fg, Rgb::new(255, 255, 255));

        // The analyzer palette is not a core role, so it is checked as it
        // lands in the file: quiet-to-loud here, loud-to-quiet in VISCOLOR,
        // so the first stop written must be VISCOLOR 17 and the last one 2.
        // What the application then makes of it is pinned on its own side.
        let ramp = file.extra["vis"]["ramp"].as_array().expect("no ramp");
        assert_eq!(ramp.len(), 16);
        assert_eq!(ramp[0].as_str(), Some("#218c00"));
        assert_eq!(ramp[15].as_str(), Some("#ef3110"));
        assert_eq!(file.extra["vis"]["peak_fg"].as_str(), Some("#ffffff"));
        assert_eq!(file.extra["vis"]["grid_fg"].as_str(), Some("#182129"));
    }

    #[test]
    fn a_skin_with_no_text_files_still_produces_a_valid_theme() {
        let toml = to_theme_toml(&SkinColors::default(), "Bare", "bare.wsz");
        let file = super::super::schema::ThemeFile::parse(&toml).unwrap();
        let t = super::super::resolve::Theme::resolve(&file);
        // Nothing was stated, so every role is derived and none is missing.
        assert!(t.bg.contrast(t.fg) > 1.0);
        assert!(
            !file.extra.contains_key("vis"),
            "a skin with no VISCOLOR.TXT should leave the analyzer to derivation"
        );
    }

    /// A skin is a zip from the internet, and deflate does not have to be
    /// honest about what it expands to.
    #[test]
    fn an_entry_that_expands_past_the_limit_is_refused_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.wsz");

        {
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            // Highly compressible, and far past the one-megabyte text limit.
            zip.start_file("VISCOLOR.TXT", options).unwrap();
            let zeros = vec![b'0'; 4 * 1024 * 1024];
            std::io::Write::write_all(&mut zip, &zeros).unwrap();
            zip.finish().unwrap();
        }

        let compressed = std::fs::metadata(&path).unwrap().len();
        assert!(
            compressed < 64 * 1024,
            "the fixture is not actually a bomb ({compressed} bytes)"
        );

        let skin = read(&path).expect("a skin with one oversized entry should still read");
        assert!(
            skin.warnings.iter().any(|w| w.contains("limit")),
            "the refusal was silent: {:?}",
            skin.warnings
        );
        // Refused rather than half-read.
        assert!(skin.viscolor.is_empty());
    }

    /// A skin's file stem becomes a TOML string, and a filename may contain a
    /// quote or a newline.
    #[test]
    fn a_skin_name_cannot_break_the_theme_file_it_is_written_into() {
        let skin = SkinColors::default();
        let hostile = "base\"\nid = \"stolen";
        let toml = to_theme_toml(&skin, hostile, "base.wsz");
        let parsed: toml::Value = toml::from_str(&toml).expect("the theme no longer parses");
        assert_eq!(parsed["meta"]["name"].as_str(), Some(hostile));
        assert_ne!(parsed["meta"]["id"].as_str(), Some("stolen"));
    }
}

/// Generated adversarial input.
///
/// Both of these read text out of a zip somebody downloaded.
#[cfg(test)]
mod fuzz {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn arbitrary_viscolor_text_never_panics(s in ".{0,2000}") {
            let (colors, _) = parse_viscolor(&s);
            prop_assert!(colors.len() <= 24, "more colours than the format has");
        }

        #[test]
        fn arbitrary_pledit_text_never_panics(s in ".{0,2000}") {
            let _ = parse_pledit(&s);
        }

        /// Lines shaped like the real files, which is where the number parsing
        /// actually gets exercised.
        #[test]
        fn colour_shaped_lines_never_panic(
            lines in proptest::collection::vec(
                proptest::sample::select(vec![
                    "0,0,0", "255,255,255", "999,999,999", "-1,-1,-1",
                    "1,2", "1,2,3,4", "a,b,c", "", "// comment",
                    "0,0,0 // comment", "4294967296,0,0", "1e10,0,0",
                    "Normal=#FF00FF", "Current=#00FF00", "NormalBG=",
                    "SelectedBG=#zzz", "=", "[Text]",
                ]),
                0..40,
            )
        ) {
            let text = lines.join("\n");
            let _ = parse_viscolor(&text);
            let _ = parse_pledit(&text);
        }
    }
}
