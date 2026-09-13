//! Following the system theme.
//!
//! Checked in order of how specific the source is. Stylix comes first because
//! it is the one that actually tracks a user's choice across the whole desktop;
//! COSMIC's own config is the fallback for a COSMIC user not running Stylix.

use std::path::PathBuf;

use super::base16::{self, Base16Scheme};

/// Where a detected system scheme came from, for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Stylix(PathBuf),
    Cosmic,
    None,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Stylix(p) => write!(f, "stylix ({})", p.display()),
            Source::Cosmic => f.write_str("cosmic"),
            Source::None => f.write_str("none"),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Candidate files, most authoritative first.
fn stylix_candidates() -> Vec<PathBuf> {
    let Some(h) = home() else { return Vec::new() };
    vec![
        h.join(".config/stylix/palette.json"),
        h.join(".config/stylix/palette.yaml"),
    ]
}

/// Is the desktop asking for a dark theme?
///
/// COSMIC records this separately from the palette, and it decides which way
/// the base16 lightness ends are read.
pub fn prefers_dark() -> bool {
    if let Some(h) = home() {
        let mode = h.join(".config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark");
        if let Ok(s) = std::fs::read_to_string(&mode) {
            return s.trim() == "true";
        }
    }
    // Dark is the right default for a terminal application.
    true
}

/// Find the system's base16 scheme, if there is one.
pub fn detect() -> (Option<Base16Scheme>, Source) {
    for path in stylix_candidates() {
        if !path.is_file() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let scheme = base16::parse(&text);
            if scheme.is_complete() {
                return (Some(scheme), Source::Stylix(path));
            }
        }
    }
    (None, Source::None)
}

/// The desktop's scheme as a theme file, if one can be detected.
///
/// A file rather than a resolved theme: each application resolves its own
/// roles out of it, and handing back one application's struct would mean the
/// other could not follow the desktop at all.
pub fn theme() -> Option<(super::schema::ThemeFile, Source)> {
    let (scheme, source) = detect();
    let scheme = scheme?;
    let variant = if prefers_dark() { "dark" } else { "light" };
    let toml = base16::to_theme_toml(&scheme, "system", variant);
    let file = super::schema::ThemeFile::parse(&toml).ok()?;
    Some((file, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_never_panics_whatever_the_machine_looks_like() {
        let (_, source) = detect();
        // Only that it produced an answer; the machine running the tests may or
        // may not have Stylix.
        let _ = source.to_string();
    }

    #[test]
    fn a_detected_scheme_resolves_to_a_usable_theme() {
        if let Some((f, _)) = theme() {
            let t = super::super::resolve::Theme::resolve(&f);
            // Whatever the scheme, body text has to be readable.
            assert!(
                t.bg.contrast(t.fg) >= 3.0,
                "system theme contrast is only {:.2}:1",
                t.bg.contrast(t.fg)
            );
        }
    }

    #[test]
    fn source_renders_something_meaningful() {
        assert_eq!(Source::None.to_string(), "none");
        assert_eq!(Source::Cosmic.to_string(), "cosmic");
        assert!(Source::Stylix("/x/p.json".into())
            .to_string()
            .contains("stylix"));
    }
}
