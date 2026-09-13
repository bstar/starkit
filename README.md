# STAR/KIT

The terminal-UI foundation shared by [STAR/AMP](https://github.com/bstar/staramp),
a Winamp-feel music player, and [STAR/CORD](https://github.com/bstar/starcord),
a Discord client. Both are terminal applications built on ratatui, both wanted
the same theme engine, the same picture rendering, the same docked panels and
the same idea of where a program's files live, and copying the answer between
them would have meant maintaining two of everything.

**This is not a general TUI framework.** Every item in it came out of one of
those two applications, and the API is whatever the two of them needed rather
than whatever a third might. That is a deliberate limit rather than an
unfinished state: a library with two known call sites can be changed by reading
both of them, and that property is worth more here than generality.

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
- **The dull necessities** — one directory for an application's files, file
  logging that never touches stdout, atomic and private writes, terminal setup
  with a panic hook that restores it, a key-binding table and the help view
  that draws it.

`ratatui`, `crossterm`, `ratatui-image` and `image` are re-exported, so both
applications use exactly one copy of each.

## Status

Early. The crate is being lifted out of STAR/AMP a module at a time, and
`CHANGELOG.md` is the honest account of what has arrived. Versions are 0.x and
tagged; both applications pin a tag.

## Licence

MIT.
