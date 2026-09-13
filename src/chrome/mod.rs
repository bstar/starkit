//! The parts of a panel that are not its contents.
//!
//! A border with a title on it, four corners tinted so the frame reads as one
//! shape, a row of action words at the top, and the overlay a panel opens to
//! change its own settings. None of it knows what the panel is for, which is
//! why it is here rather than in either application.
//!
//! Everything in here takes `&`[`Theme`](crate::theme::Theme) -- the core one.
//! An application's own theme derefs to it, so the call sites read the same as
//! they did when this was theirs.

pub mod frame;
pub mod header;
pub mod settings;

/// A built-in theme, resolved through the core, for the tests in this module.
#[cfg(test)]
pub(crate) fn test_theme(id: &str) -> crate::theme::Theme {
    use crate::theme::ThemeFile;
    let b = crate::theme::BUILTINS
        .iter()
        .find(|b| b.id == id)
        .unwrap_or_else(|| panic!("no built-in {id}"));
    crate::theme::Theme::resolve(&ThemeFile::parse(b.toml).unwrap())
}
