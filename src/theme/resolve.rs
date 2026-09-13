//! Turning a theme file into concrete colours.
//!
//! Every role a theme omits is derived from one it supplied, following a fixed
//! chain. That is what lets an eight-line theme be usable and a two-hundred-line
//! one be exact, without two code paths.
//!
//! Only the roles every application has are resolved here: the palette, the
//! chrome around a panel, the rows inside one, and the status line. An
//! application's own roles are derived from these in its own
//! [`Resolve`](super::Resolve) implementation, from the same eight colours, so
//! an analyzer and a message list dressed by one theme file agree with each
//! other.

use super::color::Rgb;
use super::schema::{ThemeFile, Variant};

/// Every colour the shared UI can ask for, all concrete.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub id: String,
    pub variant: Variant,

    pub bg: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub ok: Rgb,
    pub warn: Rgb,
    pub error: Rgb,

    pub titlebar_active_fg: Rgb,
    pub titlebar_active_bg: Rgb,
    pub titlebar_inactive_fg: Rgb,
    pub titlebar_inactive_bg: Rgb,
    pub border: Rgb,
    pub border_focused: Rgb,
    pub divider: Rgb,

    pub panel_bg: Rgb,
    pub panel_fg: Rgb,
    pub header_fg: Rgb,
    pub header_bg: Rgb,
    pub empty_fg: Rgb,

    pub row_fg: Rgb,
    pub row_bg: Rgb,
    pub row_index_fg: Rgb,
    pub row_duration_fg: Rgb,
    pub row_meta_fg: Rgb,
    pub row_selected_fg: Rgb,
    pub row_selected_bg: Rgb,
    pub row_cursor_fg: Rgb,
    pub row_cursor_bg: Rgb,
    pub row_playing_fg: Rgb,
    pub row_playing_bg: Option<Rgb>,
    pub row_marked_fg: Rgb,
    pub row_missing_fg: Rgb,
    pub row_virtual_fg: Rgb,

    pub status_fg: Rgb,
    pub status_bg: Rgb,
    pub hint_key_fg: Rgb,
    pub hint_key_bg: Rgb,
    pub hint_desc_fg: Rgb,
}

/// How far a focused panel's border is tinted toward the accent.
///
/// Low, and with a contrast floor under it, because this colour is drawn on
/// the seam between two docked panels as well as around the focused one --
/// the focused panel's bottom edge is the top of whatever is below it. Loud
/// enough to find, quiet enough that the shared edge does not read as a fault.
const FOCUS_BORDER_TINT: f64 = 0.38;

/// How far the derived selection bar is pulled back toward the background.
///
/// The contrast that matters on a selected row is between its text and its
/// bar, not between its bar and the panel; a quieter bar keeps the row legible
/// and stops the list looking like it has a hole burnt in it.
const SELECTION_DARKEN: f64 = 0.68;

pub const WHITE: Rgb = Rgb::new(255, 255, 255);
pub const BLACK: Rgb = Rgb::new(0, 0, 0);

/// The one rule the whole derivation chain is built out of: what the theme
/// said, else what its base16 scheme implies, else the fallback.
///
/// Public because an application derives its own roles the same way, out of
/// the same file, and two spellings of this would eventually be two rules.
pub fn pick(explicit: Option<Rgb>, from16: Option<Rgb>, fallback: Rgb) -> Rgb {
    explicit.or(from16).unwrap_or(fallback)
}

impl Theme {
    pub fn resolve(f: &ThemeFile) -> Self {
        // A base16 block fills in anything the theme did not state explicitly.
        //
        // No lightness flip for light schemes. base00 is *always* the default
        // background in the base16 spec -- a light scheme simply has a light
        // base00 already, as Catppuccin Latte's #eff1f5 does. Swapping the ends
        // produced lavender text on grey at 2.06:1, which the contrast test
        // caught.
        let b16 = f.base16;

        let variant = f.meta.variant;
        let default_bg = if variant == Variant::Dark {
            BLACK
        } else {
            WHITE
        };
        let default_fg = if variant == Variant::Dark {
            Rgb::new(0x96, 0x96, 0x96)
        } else {
            Rgb::new(0x30, 0x30, 0x30)
        };

        let bg = pick(f.app.bg, b16.map(|b| b.base00), default_bg);
        let fg = pick(f.app.fg, b16.map(|b| b.base05), default_fg);
        let accent = pick(f.app.accent, b16.map(|b| b.base0D), Rgb::new(0, 255, 0));
        // Halfway between fg and bg reads as "muted" rather than "faded".
        //
        // Lifted to stay readable: base16 specifies base03 as a comment colour
        // and many real schemes put it far too dark for text -- COSMIC's
        // #5A5A5A is 2.50:1 on its own background. `dim` carries hints, track
        // numbers, durations and timestamps, so it has to clear AA. An
        // explicitly stated `dim` is taken as given; only the derived one is
        // adjusted.
        let dim = match f.app.dim {
            Some(c) => c,
            None => b16
                .map(|b| b.base03)
                .unwrap_or_else(|| fg.mix(bg, 0.45))
                .ensure_contrast(bg, 4.5),
        };
        let ok = pick(f.app.ok, b16.map(|b| b.base0B), Rgb::new(0x29, 0xce, 0x10));
        let warn = pick(
            f.app.warn,
            b16.map(|b| b.base0A),
            Rgb::new(0xd6, 0xb5, 0x21),
        );
        let error = pick(
            f.app.error,
            b16.map(|b| b.base08),
            Rgb::new(0xef, 0x31, 0x10),
        );

        // A theme that states its own selection colour gets exactly that.
        // Otherwise the derived one is pulled back toward the background:
        // base02 is a *surface* colour, meant for a panel behind text rather
        // than a bar under one line of it, and used raw the selection reads as
        // a lit block rather than as a row that happens to be current.
        let sel_bg = f.row.selected_bg.unwrap_or_else(|| {
            let derived = b16
                .map(|b| b.base02)
                .unwrap_or_else(|| bg.mix(accent, 0.30));
            derived.mix(bg, SELECTION_DARKEN)
        });
        // Whichever of the obvious candidates actually reads on that background.
        //
        // base06 is *not* used blindly: it is only a light foreground in dark
        // schemes. On a light scheme like Catppuccin Latte it is a salmon
        // (#dc8a78) that sits at 1.71:1 on base02, which the contrast test
        // caught. Take it only when it genuinely reads, otherwise pick the
        // candidate that does.
        let sel_fg = f.row.selected_fg.unwrap_or_else(|| {
            let preferred = b16.map(|b| b.base06);
            match preferred {
                Some(c) if sel_bg.contrast(c) >= 4.5 => c,
                _ => sel_bg.best_contrast_against(&[fg, WHITE, BLACK]),
            }
        });

        // Chrome should frame the content, not compete with it.
        //
        // The focused border previously derived from base07, which is #FFFFFF
        // in most schemes -- so focused panels were outlined in white. Falling
        // back to the raw accent is not much better: on a scheme whose accent is
        // saturated green on black it reads as a highlight rather than a frame.
        //
        // So: a muted line with a floor so it stays visible on pure black, and a
        // focused variant that is only a gentle tint toward the accent with a
        // ceiling on how loud it can get. The step between them is deliberately
        // small -- focus should be noticeable, not shouted.
        let border = f
            .chrome
            .border
            .unwrap_or_else(|| bg.mix(fg, 0.22).ensure_contrast(bg, 1.45));
        // A tint toward the accent, and a ceiling on how far it can go.
        //
        // This was the same grey as an unfocused border for a while, because
        // two panels meet along a shared edge -- the player's floor is the
        // playlist's ceiling -- and lighting one panel's frame lights half of
        // its neighbour's, so the seam becomes a step between two greys. That
        // is still true and is the cost of this: a focused panel's bottom
        // edge is also the top of whatever sits under it.
        //
        // It is drawn anyway because a focus mark that only touches the four
        // corners is too quiet to find, which is the complaint that brought
        // this back. The tint is kept low and capped so the frame still
        // frames rather than competing with the content.
        let border_focused = f.chrome.border_focused.unwrap_or_else(|| {
            border
                .mix(accent, FOCUS_BORDER_TINT)
                // Enough of a step from the unfocused grey to read as a
                // change, whatever the theme's accent happens to be.
                .ensure_contrast(bg, 1.9)
        });

        Theme {
            name: if f.meta.name.is_empty() {
                "Unnamed".into()
            } else {
                f.meta.name.clone()
            },
            id: if f.meta.id.is_empty() {
                f.meta.name.to_lowercase().replace(' ', "-")
            } else {
                f.meta.id.clone()
            },
            variant,

            bg,
            fg,
            dim,
            accent,
            ok,
            warn,
            error,

            titlebar_active_fg: pick(f.chrome.titlebar_active_fg, None, accent),
            titlebar_active_bg: pick(
                f.chrome.titlebar_active_bg,
                b16.map(|b| b.base01),
                bg.mix(fg, 0.06),
            ),
            titlebar_inactive_fg: pick(f.chrome.titlebar_inactive_fg, None, dim),
            titlebar_inactive_bg: pick(f.chrome.titlebar_inactive_bg, None, bg),
            border,
            border_focused,
            divider: pick(f.chrome.divider, None, bg.mix(fg, 0.12)),

            panel_bg: pick(f.panel.bg, None, bg),
            panel_fg: pick(f.panel.fg, None, fg),
            header_fg: pick(f.panel.header_fg, None, accent),
            header_bg: pick(f.panel.header_bg, b16.map(|b| b.base01), bg.mix(fg, 0.06)),
            empty_fg: pick(f.panel.empty_fg, None, dim.mix(bg, 0.4)),

            row_fg: pick(f.row.fg, None, fg),
            row_bg: pick(f.row.bg, None, bg),
            // Full weight, not dim: the number and the length are the two
            // things scanned down a playlist, and dimming them made the
            // column of titles the only legible thing on the panel.
            row_index_fg: pick(f.row.index_fg, None, fg),
            row_duration_fg: pick(f.row.duration_fg, None, fg),
            row_meta_fg: pick(f.row.meta_fg, b16.map(|b| b.base04), dim),
            row_selected_fg: sel_fg,
            row_selected_bg: sel_bg,
            row_cursor_fg: pick(f.row.cursor_fg, None, sel_fg),
            row_cursor_bg: pick(f.row.cursor_bg, None, sel_bg.mix(bg, 0.35)),
            row_playing_fg: pick(f.row.playing_fg, None, accent),
            row_playing_bg: f.row.playing_bg,
            row_marked_fg: pick(f.row.marked_fg, None, warn),
            row_missing_fg: pick(f.row.missing_fg, None, error),
            row_virtual_fg: pick(f.row.virtual_fg, b16.map(|b| b.base0F), fg.mix(ok, 0.4)),

            status_fg: pick(f.status.fg, None, dim),
            status_bg: pick(f.status.bg, None, bg.mix(fg, 0.04)),
            hint_key_fg: pick(
                f.status.hint_key_fg,
                None,
                accent.best_contrast_against(&[BLACK, WHITE]),
            ),
            hint_key_bg: pick(f.status.hint_key_bg, None, accent),
            hint_desc_fg: pick(f.status.hint_desc_fg, None, dim),
        }
    }

    /// The unfilled part of a slider, a seek bar or a meter.
    ///
    /// One function rather than a field each, and computed rather than stored,
    /// because every control that has a groove wants the same grey and three
    /// copies of `bg.mix(fg, 0.18)` is three chances for one of them to drift.
    pub fn track_bg(&self) -> Rgb {
        self.bg.mix(self.fg, 0.18)
    }

    /// Every resolved colour, one `field = #rrggbb` line in declaration order.
    ///
    /// This is only here for `every_builtin_resolves_as_recorded`, which pins
    /// the whole derivation chain against `testdata/golden/`. Rewriting where
    /// resolution happens is only correct if all sixteen of those files still
    /// match byte for byte, and diffing two dumps says which role moved when
    /// they do not.
    pub fn dump(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        macro_rules! line {
            ($f:ident) => {
                let _ = writeln!(s, "{} = {}", stringify!($f), self.$f);
            };
        }
        // An unset optional role is not the same as one resolved to black, so
        // it gets a word rather than a colour.
        macro_rules! maybe {
            ($f:ident) => {
                let _ = match self.$f {
                    Some(c) => writeln!(s, "{} = {}", stringify!($f), c),
                    None => writeln!(s, "{} = none", stringify!($f)),
                };
            };
        }

        line!(bg);
        line!(fg);
        line!(dim);
        line!(accent);
        line!(ok);
        line!(warn);
        line!(error);
        line!(titlebar_active_fg);
        line!(titlebar_active_bg);
        line!(titlebar_inactive_fg);
        line!(titlebar_inactive_bg);
        line!(border);
        line!(border_focused);
        line!(divider);
        line!(panel_bg);
        line!(panel_fg);
        line!(header_fg);
        line!(header_bg);
        line!(empty_fg);
        line!(row_fg);
        line!(row_bg);
        line!(row_index_fg);
        line!(row_duration_fg);
        line!(row_meta_fg);
        line!(row_selected_fg);
        line!(row_selected_bg);
        line!(row_cursor_fg);
        line!(row_cursor_bg);
        line!(row_playing_fg);
        maybe!(row_playing_bg);
        line!(row_marked_fg);
        line!(row_missing_fg);
        line!(row_virtual_fg);
        line!(status_fg);
        line!(status_bg);
        line!(hint_key_fg);
        line!(hint_key_bg);
        line!(hint_desc_fg);

        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A built-in resolved through the core only, without going near a
    /// registry: these tests are about the derivation chain, not about where
    /// a theme was found.
    fn builtin(id: &str) -> Theme {
        let b = super::super::builtin::BUILTINS
            .iter()
            .find(|b| b.id == id)
            .unwrap_or_else(|| panic!("no built-in {id}"));
        Theme::resolve(&ThemeFile::parse(b.toml).unwrap())
    }

    fn minimal() -> ThemeFile {
        ThemeFile::parse(
            r##"
            [meta]
            name = "Minimal"
            [app]
            bg = "#000000"
            fg = "#969696"
            accent = "#00FF00"
            "##,
        )
        .unwrap()
    }

    #[test]
    fn a_minimal_theme_resolves_every_role() {
        let t = Theme::resolve(&minimal());
        assert_eq!(t.bg, Rgb::new(0, 0, 0));
        assert_eq!(t.accent, Rgb::new(0, 255, 0));
        // Derived, not defaulted to something arbitrary.
        assert_ne!(t.row_selected_bg, t.bg);
    }

    #[test]
    fn borders_are_muted_and_all_one_grey() {
        for src in [
            include_str!("../../themes/cosmic.toml"),
            include_str!("../../themes/nord.toml"),
            include_str!("../../themes/gruvbox-dark.toml"),
            include_str!("../../themes/catppuccin-latte.toml"),
        ] {
            let t = Theme::resolve(&ThemeFile::parse(src).unwrap());
            let idle = t.bg.contrast(t.border);
            let focused = t.bg.contrast(t.border_focused);

            assert!(
                idle <= 2.5,
                "{}: idle border is {idle:.2}:1, too loud",
                t.id
            );
            assert!(
                idle >= 1.2,
                "{}: idle border is {idle:.2}:1, invisible",
                t.id
            );
            assert!(
                focused <= 3.6,
                "{}: focused border is {focused:.2}:1, too loud",
                t.id
            );
            // Different from the unfocused one, and visibly so: a focus
            // mark confined to four corners is too quiet to find, which is
            // what put the tint back. The cost is real and accepted -- two
            // panels share an edge, so a focused panel's bottom border is
            // also the top of whatever sits under it.
            assert_ne!(
                t.border, t.border_focused,
                "{}: the focused border is the same as the idle one",
                t.id
            );
        }
    }

    #[test]
    fn an_idle_border_is_grey_and_a_focused_one_is_tinted() {
        // The idle frame is quiet by design. The focused one carries the
        // theme's own hue, which is what makes it findable at a glance --
        // a difference in weight alone reads as a rendering artefact rather
        // than as a state.
        for name in ["cosmic", "catppuccin-mocha", "nord", "tokyo-night"] {
            let t = builtin(name);
            let spread = |c: Rgb| c.r.max(c.g).max(c.b) as i32 - c.r.min(c.g).min(c.b) as i32;
            assert!(
                spread(t.border) <= 28,
                "{name}: the idle border is tinted, not grey: {:?}",
                t.border
            );
            assert!(
                spread(t.border_focused) > spread(t.border),
                "{name}: the focused border carries no more hue than the idle one"
            );
        }
    }

    /// Not an assertion -- run with `--nocapture` to see the two borders.
    #[test]
    fn preview_the_focus_borders() {
        for name in [
            "cosmic",
            "catppuccin-mocha",
            "nord",
            "gruvbox-dark",
            "tokyo-night",
        ] {
            let t = builtin(name);
            println!(
                "{name:18} bg {}  idle {}  focused {}  accent {}",
                t.bg.to_hex(),
                t.border.to_hex(),
                t.border_focused.to_hex(),
                t.accent.to_hex()
            );
        }
    }

    #[test]
    fn a_focused_border_is_never_plain_white() {
        // base07 is #FFFFFF in most schemes, and deriving from it outlined
        // every focused panel in white.
        let t =
            Theme::resolve(&ThemeFile::parse(include_str!("../../themes/cosmic.toml")).unwrap());
        assert_ne!(t.border_focused, Rgb::new(255, 255, 255));
    }

    #[test]
    fn selected_text_is_readable_on_the_selected_background() {
        let t = Theme::resolve(&minimal());
        let c = t.row_selected_bg.contrast(t.row_selected_fg);
        assert!(c >= 4.5, "selected row contrast only {c:.2}:1");
    }

    #[test]
    fn every_groove_is_the_same_grey() {
        // Computed once rather than stored three times: a seek bar, a volume
        // slider and an equaliser band all draw the same unfilled track, and
        // three copies of the arithmetic is three chances for one to drift.
        let t = Theme::resolve(&minimal());
        assert_eq!(t.track_bg(), t.bg.mix(t.fg, 0.18));
        assert_eq!(t.track_bg(), Theme::resolve(&minimal()).track_bg());
    }

    #[test]
    fn base16_fills_in_the_whole_palette() {
        let src = r##"
            [meta]
            name = "Stylix"
            variant = "dark"
            [base16]
            base00 = "#1e1e2e"
            base01 = "#181825"
            base02 = "#313244"
            base03 = "#45475a"
            base04 = "#585b70"
            base05 = "#cdd6f4"
            base06 = "#f5e0dc"
            base07 = "#b4befe"
            base08 = "#f38ba8"
            base09 = "#fab387"
            base0A = "#f9e2af"
            base0B = "#a6e3a1"
            base0C = "#94e2d5"
            base0D = "#89b4fa"
            base0E = "#cba6f7"
            base0F = "#f2cdcd"
        "##;
        let t = Theme::resolve(&ThemeFile::parse(src).unwrap());
        assert_eq!(t.bg, Rgb::parse_hex("#1e1e2e").unwrap());
        assert_eq!(t.fg, Rgb::parse_hex("#cdd6f4").unwrap());
        assert_eq!(t.accent, Rgb::parse_hex("#89b4fa").unwrap());
        assert_eq!(t.ok, Rgb::parse_hex("#a6e3a1").unwrap());
        assert_eq!(t.error, Rgb::parse_hex("#f38ba8").unwrap());
    }

    #[test]
    fn explicit_roles_beat_base16() {
        let src = r##"
            [meta]
            name = "X"
            [base16]
            base00 = "#1e1e2e"
            base01 = "#181825"
            base02 = "#313244"
            base03 = "#45475a"
            base04 = "#585b70"
            base05 = "#cdd6f4"
            base06 = "#f5e0dc"
            base07 = "#b4befe"
            base08 = "#f38ba8"
            base09 = "#fab387"
            base0A = "#f9e2af"
            base0B = "#a6e3a1"
            base0C = "#94e2d5"
            base0D = "#89b4fa"
            base0E = "#cba6f7"
            base0F = "#f2cdcd"
            [app]
            accent = "#00ff00"
        "##;
        let t = Theme::resolve(&ThemeFile::parse(src).unwrap());
        assert_eq!(t.accent, Rgb::new(0, 255, 0));
        assert_eq!(
            t.bg,
            Rgb::parse_hex("#1e1e2e").unwrap(),
            "base16 still fills the rest"
        );
    }

    #[test]
    fn a_light_scheme_keeps_base00_as_the_background() {
        // A genuinely light palette, so the assertion means something.
        let src = r##"
            [meta]
            name = "Latte"
            variant = "light"
            [base16]
            base00 = "#eff1f5"
            base01 = "#e6e9ef"
            base02 = "#ccd0da"
            base03 = "#bcc0cc"
            base04 = "#acb0be"
            base05 = "#4c4f69"
            base06 = "#dc8a78"
            base07 = "#7287fd"
            base08 = "#d20f39"
            base09 = "#fe640b"
            base0A = "#df8e1d"
            base0B = "#40a02b"
            base0C = "#179299"
            base0D = "#1e66f5"
            base0E = "#8839ef"
            base0F = "#dd7878"
        "##;
        let t = Theme::resolve(&ThemeFile::parse(src).unwrap());

        // base00 is the background in every scheme, light or dark. Swapping the
        // lightness ends -- which an earlier version did -- left light themes
        // at 2:1 and unreadable.
        assert_eq!(t.bg, Rgb::parse_hex("#eff1f5").unwrap());
        assert_eq!(t.fg, Rgb::parse_hex("#4c4f69").unwrap());
        assert!(
            t.bg.contrast(t.fg) >= 4.5,
            "light theme body text is only {:.2}:1",
            t.bg.contrast(t.fg)
        );
        // And the selected row has to read on a light background too.
        assert!(
            t.row_selected_bg.contrast(t.row_selected_fg) >= 4.5,
            "light theme selected row is only {:.2}:1",
            t.row_selected_bg.contrast(t.row_selected_fg)
        );
    }
}
