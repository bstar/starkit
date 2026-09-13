//! Importing base16 scheme files.
//!
//! Stylix drives this desktop from a base16 yaml, so pulling one in directly
//! means an application follows whatever the system theme is set to rather
//! than needing its colours copied by hand.
//!
//! The parser is a line scanner rather than a yaml dependency: a scheme file is
//! sixteen `baseXX: "value"` lines. The same scanner reads Stylix's
//! `palette.json`, because a flat JSON object is also `key: value` per line --
//! which means following the system theme needs no JSON dependency either.
//!
//! Handles the flat form, the newer `palette:`-nested form, quoted and
//! unquoted values, values with a leading `#`, and JSON's trailing commas.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use super::color::Rgb;

#[derive(Debug, Clone)]
pub struct Base16Scheme {
    pub name: Option<String>,
    pub author: Option<String>,
    pub colors: BTreeMap<String, Rgb>,
}

impl Base16Scheme {
    pub fn get(&self, key: &str) -> Option<Rgb> {
        self.colors.get(&key.to_ascii_lowercase()).copied()
    }

    /// A scheme needs all sixteen to be usable.
    pub fn is_complete(&self) -> bool {
        (0..16).all(|i| self.get(&format!("base{i:02X}")).is_some())
    }

    pub fn missing(&self) -> Vec<String> {
        (0..16)
            .map(|i| format!("base{i:02X}"))
            .filter(|k| self.get(k).is_none())
            .collect()
    }
}

pub fn parse(text: &str) -> Base16Scheme {
    let mut colors = BTreeMap::new();
    let mut name = None;
    let mut author = None;

    for raw in text.lines() {
        // Strip yaml comments, but only a `#` that starts a word: `#1b1b1b` is
        // a colour, not a comment, and stripping it swallowed the value.
        let line = strip_comment(raw).trim();
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim().trim_matches('"').to_ascii_lowercase();
        // JSON leaves a trailing comma, and the value may or may not be quoted.
        let val = v
            .trim()
            .trim_end_matches(',')
            .trim()
            .trim_matches(['"', '\''])
            .trim();

        match key.as_str() {
            "scheme" | "name" => name = Some(val.to_string()),
            "author" => author = Some(val.to_string()),
            k if k.starts_with("base") && k.len() == 6 => {
                if let Ok(c) = Rgb::parse_hex(val) {
                    colors.insert(k.to_string(), c);
                }
            }
            _ => {}
        }
    }

    Base16Scheme {
        name,
        author,
        colors,
    }
}

/// Remove a trailing `#` comment without eating a `#rrggbb` value.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' | b'\'' => in_quotes = !in_quotes,
            b'#' if !in_quotes => {
                // A comment marker is preceded by whitespace or starts the
                // line; `base00: #1b1b1b` has a non-space before it only when
                // it is genuinely a comment.
                let preceded_by_space = i == 0 || bytes[i - 1].is_ascii_whitespace();
                let looks_like_hex = bytes[i + 1..].iter().take(6).all(|c| c.is_ascii_hexdigit())
                    && bytes.len() >= i + 7;
                if preceded_by_space && !looks_like_hex {
                    return &line[..i];
                }
            }
            _ => {}
        }
    }
    line
}

pub fn parse_file(path: &Path) -> Result<Base16Scheme> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let scheme = parse(&text);
    if !scheme.is_complete() {
        return Err(anyhow!(
            "{} is not a complete base16 scheme; missing {}",
            path.display(),
            scheme.missing().join(", ")
        ));
    }
    Ok(scheme)
}

/// Emit a theme file that carries the scheme in its `[base16]` block.
///
/// Deliberately minimal: the derivation chain turns sixteen colours into the
/// whole palette, so writing every role out here would just be a snapshot that
/// stops tracking improvements to the derivation.
pub fn to_theme_toml(scheme: &Base16Scheme, id: &str, variant: &str) -> String {
    let name = scheme.name.clone().unwrap_or_else(|| id.replace('-', " "));
    let author = scheme.author.clone().unwrap_or_default();

    let mut s = format!(
        "# Generated from a base16 scheme.\n\
         # Every role not stated here is derived from these sixteen colours.\n\n\
         [meta]\nname = \"{name}\"\nid = \"{id}\"\n"
    );
    if !author.is_empty() {
        s.push_str(&format!("author = \"{author}\"\n"));
    }
    s.push_str(&format!("variant = \"{variant}\"\n\n[base16]\n"));

    for i in 0..16 {
        let key = format!("base{i:02X}");
        if let Some(c) = scheme.get(&key) {
            s.push_str(&format!("{key} = \"{}\"\n", c.to_hex()));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The COSMIC Dark scheme this desktop actually uses.
    const COSMIC: &str = r#"
scheme: "Cosmic Dark"
author: "System76"
base00: "1B1B1B"
base01: "2A2A2A"
base02: "3A3A3A"
base03: "5A5A5A"
base04: "A0A0A0"
base05: "C4C4C4"
base06: "BEBEBE"
base07: "FFFFFF"
base08: "F16161"
base09: "FF8985"
base0A: "DDC74C"
base0B: "7CB987"
base0C: "6296BE"
base0D: "49BAC8"
base0E: "BE6DEE"
base0F: "97D5A0"
"#;

    #[test]
    fn parses_a_real_scheme() {
        let s = parse(COSMIC);
        assert!(s.is_complete(), "missing {:?}", s.missing());
        assert_eq!(s.name.as_deref(), Some("Cosmic Dark"));
        assert_eq!(s.author.as_deref(), Some("System76"));
        assert_eq!(s.get("base00"), Some(Rgb::parse_hex("#1b1b1b").unwrap()));
        assert_eq!(s.get("base0D"), Some(Rgb::parse_hex("#49bac8").unwrap()));
    }

    #[test]
    fn keys_are_case_insensitive() {
        let s = parse(COSMIC);
        assert_eq!(s.get("BASE0d"), s.get("base0D"));
    }

    #[test]
    fn accepts_unquoted_and_hash_prefixed_values() {
        let s = parse("base00: 1b1b1b\nbase01: \"#2a2a2a\"\n");
        assert_eq!(s.get("base00"), Some(Rgb::parse_hex("#1b1b1b").unwrap()));
        assert_eq!(s.get("base01"), Some(Rgb::parse_hex("#2a2a2a").unwrap()));
    }

    #[test]
    fn reads_stylix_palette_json_without_a_json_dependency() {
        // The generic way to follow the system theme: Stylix writes this file
        // for whatever scheme is selected.
        let json = r#"{
  "base00": "1b1b1b",
  "base0D": "49bac8",
  "base0F": "97d5a0",
  "author": "System76",
  "scheme": "Cosmic Dark"
}"#;
        let s = parse(json);
        assert_eq!(s.get("base00"), Some(Rgb::parse_hex("#1b1b1b").unwrap()));
        assert_eq!(s.get("base0D"), Some(Rgb::parse_hex("#49bac8").unwrap()));
        assert_eq!(s.name.as_deref(), Some("Cosmic Dark"));
        assert_eq!(s.author.as_deref(), Some("System76"));
    }

    #[test]
    fn a_yaml_comment_is_stripped_but_a_hex_value_is_not() {
        let s = parse("base00: \"1b1b1b\"  # the background\nbase01: #2a2a2a\n");
        assert_eq!(s.get("base00"), Some(Rgb::parse_hex("#1b1b1b").unwrap()));
        assert_eq!(
            s.get("base01"),
            Some(Rgb::parse_hex("#2a2a2a").unwrap()),
            "a leading # is a colour, not a comment"
        );
    }

    #[test]
    fn accepts_the_nested_palette_form() {
        // Newer scheme files indent the colours under `palette:`.
        let s = parse("system: \"base16\"\npalette:\n  base00: \"1b1b1b\"\n  base0D: \"49bac8\"\n");
        assert_eq!(s.get("base00"), Some(Rgb::parse_hex("#1b1b1b").unwrap()));
        assert_eq!(s.get("base0D"), Some(Rgb::parse_hex("#49bac8").unwrap()));
    }

    #[test]
    fn an_incomplete_scheme_reports_what_is_missing() {
        let s = parse("base00: \"1b1b1b\"\n");
        assert!(!s.is_complete());
        assert_eq!(s.missing().len(), 15);
        assert!(s.missing().contains(&"base0D".to_string()));
    }

    #[test]
    fn the_generated_theme_resolves_to_the_scheme_colours() {
        let scheme = parse(COSMIC);
        let toml = to_theme_toml(&scheme, "cosmic-auto", "dark");
        let file = super::super::schema::ThemeFile::parse(&toml)
            .unwrap_or_else(|e| panic!("generated theme did not parse: {e}\n{toml}"));
        let t = super::super::resolve::Theme::resolve(&file);
        assert_eq!(t.bg, Rgb::parse_hex("#1b1b1b").unwrap());
        assert_eq!(t.accent, Rgb::parse_hex("#49bac8").unwrap());
        assert_eq!(t.ok, Rgb::parse_hex("#7cb987").unwrap());
        assert_eq!(t.error, Rgb::parse_hex("#f16161").unwrap());
    }
}
