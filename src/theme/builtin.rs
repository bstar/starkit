//! Themes compiled into the binary, so a fresh install has a working look
//! without shipping a data directory alongside it.
//!
//! The sixteen files are shared assets: each one dresses both applications,
//! and a table only one of them understands rides along in
//! [`ThemeFile::extra`](super::schema::ThemeFile::extra) rather than being an
//! error. [`Registry`] is the lookup both of them do -- built in, or a file in
//! the user's themes directory, or the desktop's own scheme -- parameterised
//! by the resolved type so each gets its own roles out of the same file.

use std::marker::PhantomData;

use crate::paths::Paths;

use super::schema::ThemeFile;
use super::Resolve;

pub struct Builtin {
    pub id: &'static str,
    pub toml: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "winamp-classic",
        toml: include_str!("../../themes/winamp-classic.toml"),
    },
    Builtin {
        id: "cosmic",
        toml: include_str!("../../themes/cosmic.toml"),
    },
    Builtin {
        id: "catppuccin-mocha",
        toml: include_str!("../../themes/catppuccin-mocha.toml"),
    },
    Builtin {
        id: "catppuccin-latte",
        toml: include_str!("../../themes/catppuccin-latte.toml"),
    },
    Builtin {
        id: "gruvbox-dark",
        toml: include_str!("../../themes/gruvbox-dark.toml"),
    },
    Builtin {
        id: "nord",
        toml: include_str!("../../themes/nord.toml"),
    },
    Builtin {
        id: "tokyo-night",
        toml: include_str!("../../themes/tokyo-night.toml"),
    },
    Builtin {
        id: "dracula",
        toml: include_str!("../../themes/dracula.toml"),
    },
    Builtin {
        id: "rose-pine",
        toml: include_str!("../../themes/rose-pine.toml"),
    },
    Builtin {
        id: "everforest",
        toml: include_str!("../../themes/everforest.toml"),
    },
    Builtin {
        id: "solarized-dark",
        toml: include_str!("../../themes/solarized-dark.toml"),
    },
    Builtin {
        id: "one-dark",
        toml: include_str!("../../themes/one-dark.toml"),
    },
    Builtin {
        id: "kanagawa",
        toml: include_str!("../../themes/kanagawa.toml"),
    },
    Builtin {
        id: "ayu-dark",
        toml: include_str!("../../themes/ayu-dark.toml"),
    },
    Builtin {
        id: "matte-black",
        toml: include_str!("../../themes/matte-black.toml"),
    },
    Builtin {
        id: "terminal",
        toml: include_str!("../../themes/terminal.toml"),
    },
];

pub const DEFAULT_ID: &str = "winamp-classic";

/// Where an application finds its themes.
///
/// Generic over what a theme resolves *to*, because the two applications do
/// not want the same struct out of the same file: the lookup order, the
/// fallbacks and the reason strings are shared, and the roles are not.
///
/// Cheap to construct and holds no state, so an application can keep one in a
/// `LazyLock` beside its own constants rather than threading it down from
/// `main`.
pub struct Registry<T: Resolve> {
    builtins: &'static [Builtin],
    default_id: &'static str,
    paths: Paths,
    _t: PhantomData<fn() -> T>,
}

impl<T: Resolve> Registry<T> {
    /// A registry over the themes shipped here.
    ///
    /// `paths` is the application's, because "the user's themes directory" is
    /// `~/.local/staramp/themes` for one caller and `~/.local/starcord/themes`
    /// for the other.
    pub fn new(paths: Paths) -> Self {
        Self {
            builtins: BUILTINS,
            default_id: DEFAULT_ID,
            paths,
            _t: PhantomData,
        }
    }

    /// Ship a different set. For an application that adds its own built-ins,
    /// and for tests that want a registry with nothing in it.
    pub fn with_builtins(mut self, builtins: &'static [Builtin]) -> Self {
        self.builtins = builtins;
        self
    }

    pub fn default_id(mut self, id: &'static str) -> Self {
        self.default_id = id;
        self
    }

    pub fn load(&self, id: &str) -> Option<T> {
        let b = self.builtins.iter().find(|b| b.id == id)?;
        ThemeFile::parse(b.toml).ok().map(|f| T::resolve(&f))
    }

    pub fn default_theme(&self) -> T {
        self.load(self.default_id)
            .expect("the default theme must always parse")
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.builtins.iter().map(|b| b.id).collect()
    }

    /// Resolve a theme by name, in the order a user would expect.
    ///
    /// `"system"` follows the desktop; a user theme overrides a built-in of the
    /// same id; and an unknown name falls back rather than refusing to start,
    /// because a typo in a config file should not stop the music.
    pub fn resolve_named(&self, name: &str) -> (T, String) {
        if name.eq_ignore_ascii_case("system") || name.eq_ignore_ascii_case("auto") {
            if let Some((f, source)) = super::system::theme() {
                return (T::resolve(&f), format!("system theme via {source}"));
            }
            return (self.default_theme(), "system theme not detected".into());
        }

        if let Ok(dir) = self.paths.themes_dir() {
            let path = dir.join(format!("{name}.toml"));
            if path.is_file() {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    match ThemeFile::parse(&text) {
                        Ok(f) => return (T::resolve(&f), format!("user theme {name}")),
                        Err(e) => {
                            return (self.default_theme(), format!("{name}: {e}"));
                        }
                    }
                }
            }
        }

        match self.load(name) {
            Some(t) => (t, format!("built-in {name}")),
            None => (self.default_theme(), format!("no theme `{name}`")),
        }
    }

    /// Every theme a picker should offer: built-ins, plus `system`.
    pub fn selectable(&self) -> Vec<String> {
        let mut v = vec!["system".to_string()];
        v.extend(self.ids().iter().map(|s| s.to_string()));
        if let Ok(dir) = self.paths.themes_dir() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("toml") {
                        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                            if !v.iter().any(|x| x == stem) {
                                v.push(stem.to_string());
                            }
                        }
                    }
                }
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::resolve::Theme;

    /// A registry naming an application that does not exist, so the user-theme
    /// directory it looks in is empty on every machine and the tests are about
    /// the built-ins.
    fn registry() -> Registry<Theme> {
        Registry::new(Paths::new(
            "starkit-test",
            "STARKIT_TEST_DIR",
            "STARKIT_TEST_CONFIG_DIR",
        ))
    }

    #[test]
    fn every_builtin_parses_and_resolves() {
        for b in BUILTINS {
            let f = ThemeFile::parse(b.toml)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", b.id));
            let t = Theme::resolve(&f);
            assert_eq!(t.id, b.id, "id mismatch for {}", b.id);
        }
    }

    #[test]
    fn system_resolves_to_something_usable_even_with_no_desktop() {
        let (t, why) = registry().resolve_named("system");
        assert!(t.bg.contrast(t.fg) >= 3.0);
        assert!(!why.is_empty());
    }

    #[test]
    fn an_unknown_theme_falls_back_rather_than_failing() {
        // A typo in a config file should not stop the music.
        let (t, why) = registry().resolve_named("no-such-theme");
        assert_eq!(t.id, DEFAULT_ID);
        assert!(why.contains("no theme"), "{why}");
    }

    #[test]
    fn the_selectable_list_offers_system_first() {
        let v = registry().selectable();
        assert_eq!(v.first().map(|s| s.as_str()), Some("system"));
        assert!(v.iter().any(|s| s == "cosmic"));
        assert!(v.iter().any(|s| s == "winamp-classic"));
    }

    #[test]
    fn the_default_theme_exists() {
        let t = registry().default_theme();
        assert_eq!(t.id, "winamp-classic");
    }

    // A user theme overriding a built-in of the same id is not tested here.
    // It would mean pointing the registry at a temporary directory, which
    // means setting an environment variable, which races every other test in
    // the process reading one. The application-side path is exercised by hand
    // instead: `theme list` with a file in the themes directory.

    #[test]
    fn a_registry_with_no_builtins_still_answers() {
        let reg: Registry<Theme> = registry().with_builtins(&[]);
        assert!(reg.ids().is_empty());
        assert_eq!(reg.load("cosmic").map(|t| t.id), None);
    }

    /// The whole derivation chain, pinned byte for byte.
    ///
    /// `every_builtin_is_legible` says the result is readable; this one says
    /// it is the *same* result as yesterday. It is what makes moving the
    /// resolver somewhere else a refactor rather than a rewrite, because a
    /// single changed byte is a theme that no longer looks the way it did and
    /// nothing else in the suite would notice.
    ///
    /// These are the core roles only. staramp's `testdata/theme-golden/`
    /// pins the same sixteen themes including its own, so a derivation change
    /// that only moves an analyzer colour shows up there and not here.
    ///
    /// Regenerate deliberately, after reading the diff:
    /// `STARKIT_UPDATE_GOLDEN=1 cargo test every_builtin_resolves_as_recorded`.
    #[test]
    fn every_builtin_resolves_as_recorded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/golden");
        let update = std::env::var_os("STARKIT_UPDATE_GOLDEN").is_some_and(|v| !v.is_empty());
        if update {
            std::fs::create_dir_all(&dir).expect("the golden directory has to be writable");
        }

        let reg = registry();
        for b in BUILTINS {
            let got = reg
                .load(b.id)
                .expect("a built-in that no longer loads")
                .dump();
            let path = dir.join(format!("{}.txt", b.id));

            if update {
                std::fs::write(&path, &got).expect("writing a golden file");
                continue;
            }

            let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "{}: {e} -- regenerate with STARKIT_UPDATE_GOLDEN=1",
                    path.display()
                )
            });
            if got == want {
                continue;
            }
            // The dumps are dozens of lines long, so say which role moved
            // rather than printing both of them.
            match got.lines().zip(want.lines()).find(|(g, w)| g != w) {
                Some((g, w)) => panic!("{}: resolves to `{g}`, recorded as `{w}`", b.id),
                None => panic!(
                    "{}: {} roles resolved, {} recorded",
                    b.id,
                    got.lines().count(),
                    want.lines().count()
                ),
            }
        }
    }

    #[test]
    fn every_builtin_is_legible() {
        // Body text against its own background, WCAG AA for normal text.
        let reg = registry();
        for b in BUILTINS {
            let t = reg.load(b.id).unwrap();
            let c = t.bg.contrast(t.fg);
            assert!(c >= 4.5, "{}: body text contrast is only {c:.2}:1", b.id);

            let sel = t.row_selected_bg.contrast(t.row_selected_fg);
            assert!(sel >= 4.5, "{}: selected row contrast {sel:.2}:1", b.id);

            let play = t.bg.contrast(t.row_playing_fg);
            assert!(play >= 3.0, "{}: playing row contrast {play:.2}:1", b.id);

            // dim carries hints, track numbers and durations, so it is text.
            let dim = t.bg.contrast(t.dim);
            assert!(dim >= 4.5, "{}: dim text contrast is only {dim:.2}:1", b.id);
        }
    }
}
