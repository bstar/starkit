# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
On 0.x, an API change is a minor bump and a fix is a patch.

## [Unreleased]

### Added

- **A layout tree for docked panels** (`dock`). One `layout` call per frame
  answers where every panel is, and both drawing and mouse handling read that
  answer; nothing else computes a rect. Splits, fixed and flexible sizes,
  minimums, seams that drag, hidden panels, focus order, and a config shape
  that round-trips through TOML. Placed rects always tile the area exactly;
  what does not fit is reported, not overlapped.

- **A text field** (`input`). One line or several, a cursor that steps by
  cluster so a family emoji is one backspace, the readline keys, bracketed
  paste, and a `render` that says where the terminal's own cursor goes. Any
  key it does not know is reported as such, so an application's `alt+` panel
  keys keep working while the field has focus.

- **Wrapping that reports where it cut** (`wrap`). Byte ranges per row,
  measured per cluster with `unicode-width`, so a message list can know how
  tall a message is before deciding what is on screen and which byte is
  under the pointer. With a bounded cache keyed by text and width.

- **A bottom-anchored virtual list** (`vlist`). Variable-height items,
  stick-to-end that lets go on the first upward scroll and returns when the
  end comes back into view, hit-testing that inverts the layout.

- **Animated GIF frames** (`anim`, feature `gif`). Decoded with caps on
  pixels and frames checked before anything is allocated, a 20 ms floor on
  frame delay, and the time until the next frame for a redraw loop to sleep
  on.

- **A theme file that dresses both applications** (`theme::schema`,
  `theme::resolve`). The format and the derivation chain: every role is
  optional and anything omitted is derived from what was given, so an
  eight-line theme is usable and a two-hundred-line one is exact without two
  code paths. The core resolves the tables every application has -- the
  palette, the chrome, the rows, the status line -- and keeps every other
  table verbatim in `ThemeFile::extra`, handed back by `ThemeFile::table` to
  whichever application asked for it. That is what lets one file carry
  STAR/AMP's `[vis]` and STAR/CORD's `[chat]` at once and neither see the
  other's as an error.

- **Finding a theme, once, for both of them** (`theme::Registry`). Built-in,
  or a file in the user's themes directory, or the desktop's own scheme, in
  that order, with a reason string for what it settled on and a fallback
  rather than a refusal when a config file has a typo in it. Generic over
  what a theme resolves *to*, via `theme::Resolve`, because the lookup is
  shared and the roles are not. The sixteen built-in themes come with it.

- **Following the desktop** (`theme::base16`, `theme::system`). A line scanner
  rather than a YAML dependency -- a scheme file is sixteen `baseXX: value`
  lines, and Stylix's `palette.json` is the same shape -- and the detection
  that turns one into a theme file. A file rather than a resolved theme,
  because each application resolves its own roles out of it.

- **Importing classic Winamp skins** (`theme::wsz`, feature `wsz`). A `.wsz`
  is a ZIP of bitmaps plus `VISCOLOR.TXT` and `PLEDIT.TXT`, which is where an
  imported skin's analyzer colours come from. Behind a feature because only
  one of the two applications imports skins, and it is the only thing here
  that wants a ZIP reader.

- **The key table, and the overlay that prints it** (`keymap`). `Binding<A>`
  and `MouseHelp` are the one place a key is described, so the dispatcher and
  the help screen cannot drift; `KeySpec::parse` reads the spellings such a
  table already writes for the reader (`ctrl+s`, `alt+1`, `space`, `?`, `F1`,
  `shift+left`), and `Keymap<A>` answers a key press from the table for an
  application whose dispatch is flat. A column may also describe a chord or a
  range, and those are dropped rather than refused: it is written for a person,
  and some of what a person reads is a summary. `HelpView` draws the two
  columns, group headings and all, and says in its title whether there is more
  below.

- **The parts of a panel that are not its contents** (`chrome`). The border
  decoration that makes four corners read as one frame, the row of action words
  at the top, and the settings overlay a panel opens to change itself. The
  header is generic over a `Word` the application owns, because which words a
  panel offers is the application's business and the width of them is all this
  needs to know. Words are dropped from the left as a panel narrows rather than
  the header vanishing whole: losing the way to close a panel because it got
  narrow is worse than losing the way to reorder it. One function places the
  words and both the renderer and the mouse read it, so a word that was never
  drawn cannot be clicked.

- **Pictures on a terminal that will have them** (`graphics`, feature `image`).
  Capability probing, the four modes (`auto`, `kitty`, `blocks`, `off`), the
  cell measurement a font zoom invalidates, and the half-block and `░`
  fallbacks for everything else. `Graphics::probe` must run before raw mode,
  since the terminal's reply arrives on stdin; `term::init` now records that it
  ran and probing afterwards asserts in a debug build and degrades to no
  pictures in a release one, rather than reading the user's keystrokes as the
  terminal's answer.

- **One cache for every encoded picture** (`graphics::ImageCache`). Keyed by
  what the picture *is* rather than by any name the application knows it by --
  keying a cover on its track served the second cover of an album as the first
  -- and by its placement in cells *and* in pixels, which are two different
  facts: four cells of seven pixels and two of fourteen are both twenty-eight,
  and a protocol placed over cells it was not built for leaves the rest showing
  whatever the terminal had there. Least-recently-used, because what is on the
  other side of it is the terminal's own memory, with `forget`, `forget_all`
  and a `forget_unused` for a view that knows what it just drew.

- **Shapes drawn as pixels rather than as glyphs** (`graphics::raster`). A
  polygon scan-converter that anti-aliases by supersampling the same shape test
  that decides the fill, so an edge cannot disagree with its interior. A
  terminal program cannot ship a font: a text icon is drawn by whatever typeface
  the terminal fell back to, and two machines set to different fonts draw two
  different rows of buttons from the same bytes.

- **Refusing a picture that lies about its size** (`graphics::decode_limited`).
  The `image` crate's defaults allow any dimensions and cap only the total
  allocation, at 512 MiB, so a few kilobytes claiming to be 10000x10000 turn
  into four hundred megabytes before anything notices. The header is checked
  before the pixels are read.

- **Colour and the perceptual maths under the theme engine** (`theme::color`).
  `Rgb`, hex parsing, WCAG contrast and luminance, Oklab conversion, mixing,
  ramps, and the two contrast repairs the derivation chain leans on. Blending
  in sRGB gives muddy midpoints; Oklab is perceptually uniform, which is what
  lets a theme omit a role and have the derived one look deliberate.

- **Editing one setting in a config file without disturbing the rest**
  (`config::edit`). A line editor rather than a TOML round-trip, because
  serialising the whole struct back throws away every comment, the section
  order, and any key the application does not know about. Property tests come
  with it: the file being edited is one a person typed.

- **Where an application keeps its files** (`paths::Paths`). One directory per
  application -- `~/.local/<app>`, overridable with `$<APP>_DIR` -- rather than
  three XDG roots, so a whole setup can be backed up or deleted by moving one
  folder. Const-constructible, and it names its own environment variables,
  because STAR/CORD honouring `STARAMP_DIR` would be a surprise.

- **File logging** (`logging::init`). To `<cache>/<app>.log`, filtered by
  `$<APP>_LOG` or by the `-v` flag, and covering this crate as well as the
  application: half of what an application does now happens in here, and a
  default filter naming only the application turns all of it off. Never
  stdout: it corrupts the alternate screen, and a TUI that scribbles on itself
  when something goes wrong is worse than one that says nothing.

- **Writes that cannot be half-done** (`fs::write_atomic`, `fs::write_private`).
  Temporary file in the same directory, then a rename. Everything either
  application saves while running is written over its own previous version, and
  a truncating write that is interrupted leaves a file that parses as far as it
  got. The private variant sets 0600 when the file is *created*, so a token is
  never briefly world-readable.

- **Terminal setup and teardown** (`term`). Raw mode, the alternate screen,
  mouse capture, bracketed paste, and the panic hook that undoes all of it
  before the panic is printed. Without the hook a crash leaves the user with no
  echo, no cursor and a scrambled screen, blind-typing `reset`.

- **Fitting text into columns** (`text::marquee`, `text::truncate`). Both
  measure display width rather than counting characters, because half the
  interesting titles are CJK or carry an emoji and `chars().take(n)` overflows
  the panel on one and cuts a character in half on the other.

- **Keeping a cursor on screen** (`list::clamp_scroll`). The arithmetic every
  fixed-height list in both applications was writing out for itself. A
  `VirtualList` for variable-height rows belongs beside it and is not here yet.

- **Pointer arithmetic and double clicks** (`mouse`). `hit` against a `Rect`,
  and a `ClickTracker` that pairs two clicks within 450 ms on the same cell.
  The same cell as well as the same moment: a second click a row away is a new
  selection, not an activation of the first. It clears on a pair, so a rapid
  run fires on every second click rather than on every click after the first.

- **A seven-segment font in three rows** (`digits`). Box-drawing characters
  rather than a bitmap, so it needs no font the terminal might not have.

- **The crate itself, and one copy of ratatui for both applications.**
  `ratatui`, `crossterm` and, under the `image` feature, `ratatui-image` and
  `image` are re-exported from the crate root. A widget implemented against a
  second copy of ratatui does not satisfy a signature expecting the first, and
  the compiler reports that as a mismatch between two versions carrying the
  same number. Taking all four from here makes that impossible rather than
  unlikely.
