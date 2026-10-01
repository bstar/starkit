# STAR/KIT

The terminal-UI foundation shared by [STAR/AMP](https://github.com/bstar/staramp),
a Winamp-feel music player, [STAR/CORD](https://github.com/bstar/starcord), a
Discord client, and [STAR/FOLD](https://github.com/bstar/starfold), a file
manager. All three are terminal applications built on ratatui, all three
wanted the same theme engine, the same picture rendering, the same docked
panels and the same idea of where a program's files live, and copying the
answer between them would have meant maintaining three of everything.

**This is not a general TUI framework.** Every item in it came out of one of
those three applications, and the API is whatever they needed rather than
whatever a fourth might. That is a deliberate limit rather than an unfinished
state: a library with three known call sites can be changed by reading all of
them, and that property is worth more here than generality.

## What is in it

- **Themes** — a `[meta]`/`[base16]`/role-table file format, sixteen built-in
  schemes, a derivation chain that fills in every role a file omits, Stylix and
  COSMIC detection, and Winamp `.wsz` skin import. An application extends the
  core palette with tables of its own; a theme file may carry several
  applications' tables at once.
- **Pictures** — terminal graphics through `ratatui-image` (kitty, iTerm2,
  sixel, half-blocks) with capability probing, a cache keyed by image identity
  and rectangle, and a raster path for drawing vector shapes into cells.
- **Layout** — a dockable panel tree, a virtualised list, a text-wrapping pass
  that reports where every span landed, and a text input.
- **Panel chrome** — the double-bordered frame every panel draws
  (`chrome::frame`), a scrollbar for its lists (`chrome::scrollbar`), the
  overlay box a full-screen dialog draws through (`chrome::overlay`), and a
  yes/no confirmation dialog built on it (`chrome::confirm`).
- **The dull necessities** — one directory for an application's files, file
  logging that never touches stdout, atomic and private writes, terminal setup
  with a panic hook that restores it, a key-binding table and the help view
  that draws it.

`ratatui`, `crossterm`, `ratatui-image` and `image` are re-exported, so all
three applications use exactly one copy of each.

## Status

Early. The crate is being lifted out of STAR/AMP a module at a time, and
`CHANGELOG.md` is the honest account of what has arrived. Versions are 0.x and
tagged; all three applications pin a tag.

## Licence

MIT.

## Graphical presentation experiment

The `experiment/graphical-presentation` branch adds opt-in `visual` and
`desktop` features. Default consumers retain their terminal dependency tree.
`visual` supplies theme tokens, seven vector raster icons, a bounded LRU
surface cache, explicit independent capability flags and render diagnostics.
`desktop` re-exports exactly GPUI 0.2.2 and supplies native cards, filled tabs,
menu items and capacity meters. GPUI is Apache-2.0; its transitive license
exceptions are named in `deny.toml`. No application or filesystem logic lives
here. STAR/FOLD's experimental branch is the working consumer and demo.

Build through Nix: `nix develop -c cargo test --all-features` and
`nix develop -c cargo test --no-default-features`. macOS uses GPUI runtime Metal
shader compilation, so a separate Metal compiler is unnecessary; build-time SDK
headers and libclang are still required. Linux needs the libraries listed in
the flake and CI. Keep this experiment separate from stable tags.
