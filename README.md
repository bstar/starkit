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
are exercised by the STAR apps. Responsive panes are available; consumer
adoption, a standalone starter and public API stability gates are in progress.

## What is in it

- **Default native style** — AMP's Classic Rack palette, original panel artwork,
  compact title strips, centered buttons, rows and meters through
  `native_surface::style::Style`. New apps use the same defaults with no private
  skin loader. Quiet workspace panels, aligned property fields and time rulers
  share those palette roles for larger editors. See [the native style API](docs/native-style.md).
- **Themes** — a `[meta]`/`[base16]`/role-table file format, sixteen built-in
  schemes, a derivation chain that fills in every role a file omits, Stylix and
  COSMIC detection, and Winamp `.wsz` skin import. An application extends the
  core palette with tables of its own; a theme file may carry several
  applications' tables at once. The shared Alt+T [theme picker](docs/themes.md#shared-picker)
  provides search, keyboard navigation, palette previews and native/cell rendering.
- **Pictures** — terminal graphics through `ratatui-image` (kitty, iTerm2,
  sixel, half-blocks) with capability probing, a cache keyed by image identity
  and rectangle, and a raster path for drawing vector shapes into cells.
- **Layout** — responsive nested rows/columns with wrapping, minimum/preferred/maximum
  sizes, priority collapse, explicit visibility reasons, shared focus and drag capture
  (`panes`), plus a legacy dock tree, a virtualised list and a text-wrapping pass
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

Applications pin immutable git tags for releases and commit revisions for
development candidates, with git-sourced lockfiles. The current framework
release is `v0.8.1`; development main uses the unreleased 0.9 candidate.
See [the changelog](CHANGELOG.md). CI checks all
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

## Start an application

Copy [`examples/starter`](examples/starter) into a new repository. It is an
independent package pinned to an immutable KIT candidate revision and uses only
public APIs. No sibling checkout is required.
Run through KIT's development shell while working here:

```sh
nix develop -c cargo run --manifest-path examples/starter/Cargo.toml -- --inspect
nix develop -c cargo run --manifest-path examples/starter/Cargo.toml
nix develop -c cargo run --manifest-path examples/starter/Cargo.toml --features native -- --native
```

`--ssh HOST REMOTE_EXECUTABLE` attaches the same native/cell frontend to a copy
installed on another machine. KIT authenticates before raw mode. No media
backend is required for these UI paths. `--features media` enables codecs and
speaker output when an application uses KIT's media transport.

The starter's `--inspect` emits capabilities, fallback reasons, pane rectangles,
visibility and focus as JSON. `--replay` accepts a bounded JSON action array on
stdin and emits one deterministic snapshot per action. For example:

```json
[{"action":"resize","columns":35,"rows":20},{"action":"step","forward":true}]
```

`diagnostics::Replay` supplies this behavior to any application without owning
its domain state. Reports contain presentation metadata only. Applications
should redact their own content before adding it to a report.

## Presentation and dependency boundaries

The **unreleased 0.9 candidate** also supports engine-owned live content.
`terminal_graphics::live_canvas::LiveFrames` accepts RGBA/BGRA frames from an
application worker or compositor. A controller drains its updates through
`Controller::live_frames`; `Component::LiveCanvas` places each stream with a
`content_view::ContentView`. The same fractional transform maps pointer input
back to content coordinates. Scene artwork after the canvas appears above it.
Domain capture, playback, editing and export stay in the application.

The local frontend imports private immutable mapped files and acknowledges each
generation/sequence. Each stream coalesces to its newest pending frame and keeps
one mapped frame in flight; eight stream identities and 256 MiB per transport
bound resources. A pause does not expire an unacknowledged frame. Frontends skip
already-retired descriptors from older controllers while keeping usable pixels.
A frame is at most 8192 pixels per dimension, 32 million pixels
and 128 MiB. A close retires its generation; reopening requires a higher one.
This path currently requires local Kitty graphics. Remote sessions never import
these host paths, and a graphics-only consumer does not acquire FFmpeg or CPAL.

Clicking an interactive canvas captures embedded keys. Ctrl+Esc, focus loss,
resize, a toolbar click and detach release capture. `Input::Embedded` includes
monotonic timestamps, sequence numbers, key press/repeat/release, source-space
pointer events and bounded paste text. Reliable held keys require the terminal's
extended keyboard support. Relative mouse/pointer lock is not implemented.

`application::Application::new("app")?.run(factory)` owns local controller/relay
startup and original domain arguments. `--attach SESSION` reattaches after a
frontend exits. Controllers and their workers survive frontend loss. Existing
local/SSH launcher APIs remain available. `run_or_attach(factory, matcher)` lets
an application reopen a matching resource automatically; the matcher receives
the retained arguments and original launch directory. Legacy argument files
remain readable. Startup failures are logged and sent through the protocol to
the frontend, which restores the terminal before displaying their cause.
The frontend reports its OSC 11 default color in the optional
`Capabilities::terminal_background` field. The bounded startup query reads the
presenting terminal, including SSH frontends; missing support leaves it `None`.
Applications choose their own fallback and background policy.
`worker::Worker` provides cooperative
cancellation and bounded progress/completion; dropping it cancels without
blocking the UI. Applications must make jobs honor cancellation. Reusable
`editor_controls` draw controls and map time; they do not own timeline models.

[`examples/live-canvas`](examples/live-canvas) is an independent two-engine
consumer pinned to an immutable 0.9 candidate revision. Use
`nix develop -c python3 scripts/check-live-canvas.py` to build an external copy
against current local changes before publishing another candidate. `scripts/test-live-canvas.py` exercises actual
Kitty animation, resize, frontend loss, reattachment and cleanup. Tiled Wayland
uses terminal font zoom for the viewport resize check.

| Requirement | KIT feature/API |
| --- | --- |
| Themes, responsive panes, input, focus, ASCII and reduced colors | No default features; `panes`, `presentation`, `chrome`, `term::Session` |
| Image decoding and terminal image protocols | `image`, optional `gif`, `webp`, `bmp` |
| Native Rust renderer and local/SSH scenes | `terminal-graphics`; no FFmpeg or audio device libraries |
| Source-format PCM and precision conversion | `pcm`; no device required |
| Optional local/relayed speaker output | `audio-output`; FFmpeg, CPAL and a bounded ring |
| Posters, track discovery, video decode and streaming | `media`; add `terminal-graphics` for the frontend |
| Transport buttons, volume and track picker | `media_controls`; no optional features or subprocess |

`presentation::Environment` contains sanitized environment hints;
`Observed` contains probe/negotiation evidence. Resolving a profile never probes
or starts a device. Unknown graphics fall back to Unicode cells or ASCII.
Kitty, Sixel and iTerm2 image support are separate from native UI rendering,
pixel mouse input, trusted host paths and speaker access. An ordinary SSH shell
cannot reach the terminal computer's speakers; use the local SSH frontend for
that route. Device-open errors remain explicit and never switch playback hosts.

`STAR_KIT_ASCII=1` requests ASCII presentation; `NO_COLOR` disables cell colors.
`Profile::adapt_buffer` converts shared chrome/content to the selected glyph and
color limits. The graphical session's cell frontend applies it automatically.
Ordinary ratatui applications apply it once after painting their buffer, as the
starter does. Use non-color modifiers for selection in monochrome themes.

## Readiness and release gates

KIT remains a versioned 0.x public preview. Published tags and portable locks,
four consumer checks, full/minimal/graphics-only/audio feature tests, deterministic
layout/replay tests, native rendering tests and an independent starter protect
its public boundaries. Packaging is checked with `cargo package`; release tags
do not imply a crates.io publication or a 1.0 compatibility guarantee.

Linux Kitty graphics and FOLD/AMP integration have private terminal capture
fixtures. Linux audio conversion and movie decoding have automated fixtures;
listening quality still requires a person. macOS builds have CI coverage, but
physical Mac rendering, audio devices and sustained SSH listening remain release
acceptance work. Sixel/iTerm2 implementations have fallback tests and still need
physical terminal acceptance. Do not infer tested behavior from a feature flag.

Use `scripts/check-starter.py --candidate` for an external-package check before
releasing, and omit `--candidate` to verify the published pin afterwards.
`examples/responsive-panes --bench` measures changing viewports; the native
`terminal-graphics` example changes rendered pixels on every iteration. Local
results and explicit limits belong with release evidence; throughput varies by
font, terminal, frame size and hardware.

`check-performance.py` enforces broad regression limits in release mode:
10 microseconds per changing pane layout and 100 ms native scene-to-frame p95
for the fixed 120-frame fixture. These deliberately generous CI limits detect
major regressions; they are not latency targets or proof of 60 Hz rendering.
Use lower explicit budgets when testing target hardware. The benchmark must
send a nonzero changed-pixel payload after the initial frame.
