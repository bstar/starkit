# STAR/KIT

A Rust framework for beautiful, efficient terminal applications, developed in
[STAR/AMP](https://github.com/bstar/staramp),
[STAR/FOLD](https://github.com/bstar/starfold),
[STAR/CORD](https://github.com/bstar/starcord) and
[STAR/WIRE](https://github.com/bstar/starwire).

KIT combines a Ratatui cell presentation with native pixel rendering, terminal
capability negotiation, SSH scene transport, audio output and video streaming.
Applications keep their domain models and commands. KIT supplies the shared
presentation and transport foundation, with capability-based fallbacks.

The public framework is in development. Versions remain 0.x; shared engines
are exercised by the STAR apps, while unified responsive panes, a standalone
starter and the public API stability gates are still being developed.

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
applications use exactly one copy of each.

## Status

Applications pin immutable git tags and lock the resolved revision. The current
alignment release is `v0.6.1`; see [the changelog](CHANGELOG.md). CI checks all
four consumers against the candidate checkout as well as KIT's full and minimal
feature sets. This catches shared API regressions before the next tag.

Responsive pane adoption and application migrations happen incrementally.
Existing app-specific layouts remain supported until a shared replacement has
proven its interaction, visual fidelity and performance in a real consumer.

## Licence

MIT.


## Experimental graphics inside Kitty

The `terminal-graphics` feature provides a shared native Rust
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
SSH streams use 32 bounded decoded video frames of lookahead so the shared
demuxer can refill audio while video waits for the clock.
Video uses a continuous clock anchored to the audio device's playback timestamp,
including output latency, rather than jumping by a whole audio callback period.
The terminal frontend polls every 2 ms during playback and returns to its quiet
poll while paused or idle.
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
