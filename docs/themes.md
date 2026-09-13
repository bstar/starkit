# Themes

A theme is one TOML file, read by every application that uses this crate. The
core resolves the roles they all have; each application resolves its own out of
the same file.

## The file

```toml
[meta]
name    = "Tokyo Night"
id      = "tokyo-night"      # defaults to the name, lowercased and hyphenated
author  = "enkia"
variant = "dark"             # or "light"

[base16]                     # optional, and usually the whole theme
base00 = "#1a1b26"
# ... base01 through base0F

[app]                        # the eight colours everything else derives from
bg = "#1a1b26"
fg = "#c0caf5"
dim = "#565f89"
accent = "#7aa2f7"
ok = "#9ece6a"
warn = "#e0af68"
error = "#f7768e"

[chrome]                     # titlebars, borders, dividers
[panel]                      # panel background, header, empty state
[row]                        # list rows: selected, cursor, playing, marked
[status]                     # the status line and its key pills
```

Every role is optional. Thirteen of the sixteen built-ins are `[meta]` plus
`[base16]` and nothing else; the rest of the palette is derived. A theme that
wants an exact colour states it and the derivation leaves it alone.

Colours are `#rrggbb` or `#rgb`. Anything else is a parse error naming the
value, because a theme file is something a person typed.

## Derivation

`Theme::resolve` fills a role in this order: what the file stated, then what
its `[base16]` block implies, then a fallback derived from the palette. The
mixing is done in Oklab (`theme::color`), which is why a 30% mix of a dark
background and a bright accent reads as 30% of the way there rather than as
mud.

Two derived roles are repaired against WCAG contrast rather than taken as
given:

- `dim` carries hints, track numbers, durations and timestamps, so it is text
  and has to clear 4.5:1 on the background. base16 specifies `base03` as a
  comment colour and real schemes often put it far below that.
- The selected row's foreground is whichever of `base06`, `fg`, white and
  black actually reads on the selected background. `base06` is a light
  foreground in dark schemes and a salmon in light ones.

`every_builtin_is_legible` enforces the first of those across all sixteen
built-ins, and `testdata/golden/` records what each one resolves to role by
role. A deliberate change to the derivation is a diff of colours in that
directory; an accidental one is a failing test.

## Application tables

The core deserialises `meta`, `base16`, `app`, `chrome`, `panel`, `row` and
`status`. Every other table is kept verbatim in `ThemeFile::extra`, so one file
can carry STAR/AMP's `[vis]` and STAR/CORD's `[chat]` at once and neither
application sees the other's table as an error.

An application reaches its own with `ThemeFile::table`, and extends the core
theme by holding one:

```rust
pub struct Theme {
    core: starkit::theme::Theme,
    pub vis_ramp: [Rgb; 16],
    // ...
}

impl Deref for Theme {
    type Target = starkit::theme::Theme;
    fn deref(&self) -> &Self::Target { &self.core }
}

impl starkit::theme::Resolve for Theme {
    fn resolve(f: &ThemeFile) -> Self {
        let core = starkit::theme::Theme::resolve(f);
        let vis: VisColors = f.table("vis").unwrap_or_default();
        // derive the rest from core.bg, core.accent, f.base16, ...
    }
    fn core(&self) -> &starkit::theme::Theme { &self.core }
}
```

Deriving from the core's already-resolved palette rather than from the file
again is what keeps an analyzer and a playlist in colours that agree. A
malformed application table is worth treating as absent: the rest of the theme
is fine, and refusing to start over a mistyped analyzer colour is not a trade
anyone would make.

## Finding one

`Registry<T>` is the lookup, generic over what a theme resolves to:

```rust
static REGISTRY: LazyLock<Registry<Theme>> = LazyLock::new(|| Registry::new(PATHS));
```

`resolve_named` tries, in order:

1. `"system"` or `"auto"` — the desktop's own scheme, via Stylix's
   `palette.json`/`palette.yaml` (`theme::system`).
2. `<config dir>/themes/<name>.toml` — a user theme, which overrides a
   built-in of the same id.
3. A built-in.
4. The default, `winamp-classic`, with a reason string saying why.

It never fails. A typo in a config file should not stop the application, so the
second return value is a sentence to show rather than an error to propagate.

## Importing

- `theme::base16` reads a base16 scheme — the flat form, the `palette:`-nested
  form, and Stylix's JSON, with one line scanner and no YAML dependency — and
  writes it back out as a `[base16]` theme file.
- `theme::wsz` (feature `wsz`) reads a classic Winamp skin. `VISCOLOR.TXT` and
  `PLEDIT.TXT` are where the colours are; the bitmaps are only sampled for the
  marquee's two tones. Everything a skin omits falls through to the ordinary
  derivation, so a partial skin still makes a complete theme.

Both write a file rather than a struct, so an imported theme is something the
user can open and edit.
