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


## Experimental graphics inside Kitty

The `experiment/terminal-graphics` branch adds an optional shared native Rust
renderer and persistent SSH scene relay. Neither the local renderer nor remote application needs a browser or display server. See [terminal graphics](docs/terminal-graphics.md) for build,
launch, verification and compatibility details. The ordinary TUI remains the
standard build.

### Experimental native media previews

The `media` feature provides bounded native FFmpeg poster decoding. The
`terminal-graphics` feature additionally provides local video scheduling, CPAL
audio, a native timeline surface and credit-based H.264/AAC preview streaming
over the existing SSH scene transport. Codec/audio contexts stay on workers;
the pixel renderer receives RGBA frames without serializing scenes per frame.
Local movies decode audio on an independent worker with a bounded half-second
ring and a 100 ms startup reserve, so video conversion cannot block audio refill.
SSH streams use eight bounded decoded video frames of lookahead so the shared
demuxer can refill audio while video waits for the clock.
Callbacks consume complete channel frames; underruns are counted and logged
outside the callback. Applications own file selection and lifecycle, not codec
or audio backends.

For a muted test against the actual output device:

```sh
nix develop -c cargo run --release --features media --example movie-bench -- MOVIE --audio --seconds=20 --start=600
```

Use `--stream --audio` with an MPEG-TS proxy file to check streamed playback.
This checks for buffer underruns; a listening check is still needed to judge
sound quality.

Build optional media through `nix develop` (FFmpeg, clang/libclang, pkg-config;
ALSA and matching audio plugins on Linux). `--no-default-features` continues to
link no decoder. Physical macOS audio/Kitty verification remains outstanding.
