//! Themes: the file format, and turning one into concrete colours.
//!
//! A theme file is one file for both applications. The core resolves the
//! tables it knows -- `meta`, `base16`, `app`, `chrome`, `panel`, `row`,
//! `status` -- into [`Theme`], and each application resolves its own roles out
//! of the same file by implementing [`Resolve`], reading the palette the core
//! derived and its own table out of [`ThemeFile::table`]. That is what keeps
//! staramp's analyzer and starcord's message list in the same colours when
//! they are dressed by the same file, and what lets a file carry a table only
//! one of them understands.

pub mod base16;
pub mod builtin;
pub mod color;
pub mod resolve;
pub mod schema;
pub mod system;
// A Winamp skin is a ZIP of bitmaps, which is a decoder and an archive reader
// that only the application importing skins has any use for.
#[cfg(feature = "wsz")]
pub mod wsz;

pub use builtin::{Builtin, Registry, BUILTINS, DEFAULT_ID};
pub use resolve::{pick, Theme, BLACK, WHITE};
pub use schema::{ThemeFile, Variant};

/// What a theme file resolves to.
///
/// Implemented once here for the core [`Theme`] and once in each application
/// for its own extended one, which holds the core by value and derives the
/// rest from it. [`Registry`] is generic over this, so the lookup order, the
/// fallbacks and the reason strings are written once.
pub trait Resolve: Sized + Clone {
    fn resolve(file: &ThemeFile) -> Self;

    /// The shared roles, for code that takes any application's theme.
    fn core(&self) -> &Theme;
}

impl Resolve for Theme {
    fn resolve(file: &ThemeFile) -> Self {
        Theme::resolve(file)
    }

    fn core(&self) -> &Theme {
        self
    }
}

/// A theme id that is a file name and nothing else.
///
/// The id becomes `<themes dir>/<id>.toml`, and the name it is derived from
/// comes from `--name` or from a skin's file stem. A separator or a `..` in
/// there is a path, not a name.
pub fn safe_id(name: &str) -> anyhow::Result<String> {
    let id = name.trim().to_lowercase().replace(' ', "-");
    anyhow::ensure!(
        !id.is_empty()
            && id != "."
            && id != ".."
            && !id.contains(['/', '\\', '\0'])
            && !id.starts_with('.'),
        "a theme name must be a plain file name, and {name:?} is not"
    );
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_id_is_a_file_name_and_nothing_else() {
        assert_eq!(safe_id("Tokyo Night").unwrap(), "tokyo-night");
        for hostile in ["", "..", ".", "../../etc/passwd", "a/b", "a\\b", ".hidden"] {
            assert!(safe_id(hostile).is_err(), "{hostile:?} was accepted");
        }
    }
}
