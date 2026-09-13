//! Theme file format.
//!
//! Every colour role is optional. A minimal theme is a handful of lines and the
//! rest is derived; a maximal one specifies everything. That is the opposite of
//! the reference implementation, whose seven colours cannot express "selected
//! row" and "playing row" as different things, and where one `accent` value
//! drives the title, the selection, the seek bar and the key pills at once.
//!
//! Only the tables every application shares are named here. A theme file is
//! read by both of them, so anything else it carries -- staramp's `[vis]`, or
//! starcord's `[chat]` -- is kept verbatim in [`ThemeFile::extra`] and handed
//! back by [`ThemeFile::table`] to whichever one asked for it. A resolver that
//! rejected unknown tables would make every theme file the property of one
//! application.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::color::Rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub variant: Variant,
    /// Inherit from another theme, then override.
    #[serde(default)]
    pub extends: Option<String>,
    /// Where an imported theme came from, e.g. a `.wsz` filename.
    #[serde(default)]
    pub source: Option<String>,
}

/// A base16 scheme, which can stand in for the whole palette.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct Base16 {
    pub base00: Rgb,
    pub base01: Rgb,
    pub base02: Rgb,
    pub base03: Rgb,
    pub base04: Rgb,
    pub base05: Rgb,
    pub base06: Rgb,
    pub base07: Rgb,
    pub base08: Rgb,
    pub base09: Rgb,
    pub base0A: Rgb,
    pub base0B: Rgb,
    pub base0C: Rgb,
    pub base0D: Rgb,
    pub base0E: Rgb,
    pub base0F: Rgb,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppColors {
    pub bg: Option<Rgb>,
    pub fg: Option<Rgb>,
    pub dim: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub ok: Option<Rgb>,
    pub warn: Option<Rgb>,
    pub error: Option<Rgb>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChromeColors {
    pub titlebar_active_fg: Option<Rgb>,
    pub titlebar_active_bg: Option<Rgb>,
    pub titlebar_inactive_fg: Option<Rgb>,
    pub titlebar_inactive_bg: Option<Rgb>,
    pub border: Option<Rgb>,
    pub border_focused: Option<Rgb>,
    pub divider: Option<Rgb>,
    #[serde(default)]
    pub border_style: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RowColors {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub index_fg: Option<Rgb>,
    pub duration_fg: Option<Rgb>,
    pub meta_fg: Option<Rgb>,
    pub selected_fg: Option<Rgb>,
    pub selected_bg: Option<Rgb>,
    pub cursor_fg: Option<Rgb>,
    pub cursor_bg: Option<Rgb>,
    pub playing_fg: Option<Rgb>,
    pub playing_bg: Option<Rgb>,
    pub marked_fg: Option<Rgb>,
    pub missing_fg: Option<Rgb>,
    pub virtual_fg: Option<Rgb>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PanelColors {
    pub bg: Option<Rgb>,
    pub fg: Option<Rgb>,
    pub header_fg: Option<Rgb>,
    pub header_bg: Option<Rgb>,
    pub empty_fg: Option<Rgb>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusColors {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub hint_key_fg: Option<Rgb>,
    pub hint_key_bg: Option<Rgb>,
    pub hint_desc_fg: Option<Rgb>,
}

/// A theme file, before derivation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeFile {
    pub meta: Meta,
    #[serde(default)]
    pub base16: Option<Base16>,
    #[serde(default)]
    pub app: AppColors,
    #[serde(default)]
    pub chrome: ChromeColors,
    #[serde(default)]
    pub panel: PanelColors,
    #[serde(default)]
    pub row: RowColors,
    #[serde(default)]
    pub status: StatusColors,
    /// Everything the core does not know about, kept rather than rejected.
    ///
    /// This is what lets one file dress both applications: staramp reads
    /// `[vis]` out of here and starcord reads `[chat]`, and neither sees the
    /// other's table as an error.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl ThemeFile {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// One of the application's own tables, deserialised on demand.
    ///
    /// A table the file does not have is [`Default`] rather than an error: a
    /// theme that says nothing about the analyzer is a theme whose analyzer is
    /// derived, which is the whole point of the format.
    pub fn table<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, toml::de::Error> {
        match self.extra.get(name) {
            Some(v) => T::deserialize(v.clone()),
            None => Ok(T::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Deserialize, PartialEq)]
    struct Vis {
        bg: Option<Rgb>,
        #[serde(default)]
        grid: Option<String>,
    }

    #[test]
    fn an_unknown_table_is_kept_rather_than_refused() {
        // cosmic.toml has carried a table the core knows nothing about since
        // before there was a second application, which is the reason this is
        // the design rather than a concession to one.
        let f = ThemeFile::parse(
            r##"
            [meta]
            name = "X"
            [app]
            bg = "#000000"
            [vis]
            bg = "#010203"
            grid = "dots"
            "##,
        )
        .unwrap();
        assert_eq!(f.app.bg, Some(Rgb::new(0, 0, 0)));
        let vis: Vis = f.table("vis").unwrap();
        assert_eq!(vis.bg, Some(Rgb::new(1, 2, 3)));
        assert_eq!(vis.grid.as_deref(), Some("dots"));
    }

    #[test]
    fn a_table_the_file_omits_is_the_default_one() {
        let f = ThemeFile::parse("[meta]\nname = \"X\"\n").unwrap();
        let vis: Vis = f.table("vis").unwrap();
        assert_eq!(vis, Vis::default());
    }

    #[test]
    fn a_table_that_does_not_fit_is_an_error_rather_than_a_default() {
        // Silently defaulting would turn a typo in somebody's theme into a
        // colour they did not choose and cannot find.
        let f = ThemeFile::parse("[meta]\nname = \"X\"\n[vis]\nbg = \"not a colour\"\n").unwrap();
        assert!(f.table::<Vis>("vis").is_err());
    }

    #[test]
    fn the_core_tables_are_not_swallowed_by_the_flattened_rest() {
        let f = ThemeFile::parse(
            r##"
            [meta]
            name = "X"
            [status]
            fg = "#112233"
            "##,
        )
        .unwrap();
        assert_eq!(f.status.fg, Some(Rgb::new(0x11, 0x22, 0x33)));
        assert!(f.extra.is_empty(), "{:?}", f.extra);
    }
}
