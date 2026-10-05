# Native graphics inside Kitty

The experimental `terminal-graphics` feature renders entirely in Rust. It uses
`tiny-skia` for vector drawing and `cosmic-text`/Swash for shaped, antialiased
text. There is no Electron, Chromium, webview, JavaScript, HTML or CSS runtime.
The earlier browser experiment is preserved in Git history only.

## Shared foundation

STAR/KIT owns scenes, typography, icons, panels, menus, tabs, meters, raster
previews, bounded rendering queues, Kitty image placements and terminal input.
Applications retain their existing controllers, commands, file workers and hit
geometry. The ordinary TUI feature matrix is unchanged.

```text
Local Kitty ← changed PNG regions ← native Rust pixel renderer
                                      ↑
                               STAR/KIT frontend
                                      ↕ versioned scene/input protocol
                         local relay or ordinary SSH
                                      ↕
                      persistent Rust application controller
```

Neither end requires a browser, GPU, window system or display server. Rendering
runs in one local Rust thread. Kitty provides the final display. The remote
controller never draws a browser window and only sends visible scene data and
changed preview assets. SSH authentication finishes before raw terminal mode.

## Rendering and limits

The worker keeps one pending scene and two completed frames. Native RGBA buffers
pass directly to the presenter: there is no full-frame PNG encode/decode hop.
Only changed terminal regions are encoded and uploaded; old placements retire
within a synchronized terminal update. Presentation acknowledgements follow
terminal writes, preserve generation guards and coalesce remote scenes.

The presenting client queries Kitty's current PostScript font name and point size
using [XTGETTCAP](https://sw.kovidgoyal.net/kitty/kittens/query_terminal/), and
resolves that face in its local font database. Text throughout the native UI uses
this face with natural shaping; font size is converted using Kitty's reported DPI.
The bounded query runs before UI input begins and needs no remote-control permission.
SSH attachments use the local client's fonts, not fonts on the remote host.
`STAR_GRAPHICS_FONT="JetBrainsMono Nerd Font"` overrides the family (PostScript
names also work); `STAR_GRAPHICS_FONT_SIZE=16` overrides the raster size in pixels.
Without a usable query/font, an installed monospace face or bundled Liberation
Mono supplies the fallback. Rasterization uses Swash, so antialiasing/hinting can
differ from Kitty even with the same font and size. Glyph coverage is composited
in linear light with sRGB output, matching the modern Linux Kitty default’s
blending model ([renderer reference](https://github.com/kovidgoyal/kitty/blob/master/kitty/shaders/cell.slang)).
This preserves light strokes on dark themes without making every label bold.
Kitty’s custom contrast settings and FreeType/CoreText hinting are not replicated. Terminal font zoom follows
cell-size changes; a changed font family takes effect on reattachment.
Bundled Liberation fonts provide startup without a required font installation. Their SIL Open Font License is included
in `assets/fonts/LICENSE`. Installed fonts provide Unicode fallback on Linux
and macOS. `STAR_GRAPHICS_SYSTEM_FONTS=0` disables discovery for reproducible
font tests; scripts needing CJK or emoji should leave discovery enabled.
Fallback coverage depends on installed fonts.

Text is shaped as runs, cached, clipped to its region and kept on one line for
file labels. Lists show only visible entries. Password fields draw masked text.
Decoded and resized previews survive cursor/menu/progress repaints and are
released when no longer referenced. Asset IDs are scoped to the session epoch.

Viewports remain bounded to 8192 pixels per dimension, 32 megapixels, and
120,000 logical cells. Protocol messages are bounded to 16 MiB. Preview assets
are validated before decoding, at most eight are active, decoded preview cache
is capped at 64 MB and resized preview cache at 128 MB. Text and glyph caches
are bounded. Invalid geometry/assets return an error; terminal guards restore
the terminal. Worker failure leaves the remote session intact for reattachment.

The graphical layout defaults to 100% scale. `STAR_GRAPHICS_SCALE=150` enlarges
the layout; values 100–200 are accepted. Input and OSC 72 coordinates
are translated to the logical layout while image placements retain the terminal's
physical cell grid. Existing 60×21 layout limits remain.

## Build and verify

```sh
nix develop -c cargo test --all-features
nix develop -c cargo test --no-default-features
nix develop -c cargo run --release --example terminal-graphics --features terminal-graphics -- /tmp/starkit-native.png
STAR_GRAPHICS_BENCH_FRAMES=30 target/release/examples/terminal-graphics /tmp/starkit-native-benchmark.png
```

The PNG example is headless and requires no Electron or Node installation.
The benchmark changes a selected row; its renderer timing is distinct from
full application input-to-display latency.

`scripts/test-kitty-interactive.py` verifies actual image pixels, keyboard and
pointer input, menus, font resize, clean exit, and frontend detachment retaining
session state. It requires Kitty >=0.49 for its screenshot API, Python and
Pillow. Linux CI runs it under Xvfb; Xvfb is needed by Kitty, not the renderer.
Mac headless raster tests run in CI. Interactive Kitty on macOS still requires
a real logged-in Mac: hosted macOS cannot provide Kitty's OpenGL surface.

`scripts/test-fractional-background.py` checks blank span backgrounds across
fractional cell boundaries. Native tests cover asset reuse, resize, eviction,
clipped Unicode file labels, password masking and invalid data. Presentation
tests reconstruct skipped frames, resize and complete-frame transitions.

## Compatibility

| Environment | Presentation |
| --- | --- |
| Kitty with graphics protocol | Native Rust pixel UI inside the terminal |
| Ghostty advertising Kitty graphics | Same native renderer and transport; reverify desktop behavior after this replacement |
| Kitty over SSH | Local native rendering, remote Rust controller and workers |
| tmux / terminal without Kitty graphics | Shared controller presented as terminal cells |
| No graphical display server | Raster rendering and SSH controller work; the terminal supplies its own display |

Clipboard effects use the terminal's OSC 52 protocol, with tmux buffer support
when needed. Desktop file drag/drop remains a separately detected OSC 72
extension; Kitty image support alone does not imply desktop drag/drop support.

## References

Awrit informed terminal image placement and local/remote presentation boundaries;
its browser implementation is not used. The native raster architecture follows
Rust drawing and shaping APIs:

- https://github.com/linebender/tiny-skia
- https://github.com/pop-os/cosmic-text

### Pixel region layout

Frontends negotiate `pixel_layout` separately from `native_surfaces`. A scene can
then place logical controller regions into bounded pixel rectangles using
`placement::Placement`. Shared column/split helpers use physical gaps; the same
placement maps a terminal cell centre back into controller coordinates. Gutters
have no hit target and scrollbar captures clamp to the originating region.
Pointer precision remains **cells**.

Layers are rasterized directly at their destination size, preserving the
terminal-derived font size and the shared image cache. No finished text bitmap
is resized. Modal spans belong to a separate topmost layer; popup components follow the
first Menu/Dialog marker, including their scrollbars. Moving a popup
cannot leave its old text in the background. Legacy clients retain the cell
layout. Placements are limited to 16 regions and two viewport areas of painting.

Native session tabs support an optional one-based `number`, rendered separately
from the label. Active sessions use a soft filled rectangle and a bold label;
inactive sessions remain unfilled and muted. Close/navigation/add controls keep
their original independent hit targets. There is no tab underline.

### Local file capabilities for graphical SSH drops

For a Linux client launched with `Launch.host`, the extension-aware frontend
advertises its own OSC 72 machine identity to Kitty. Only a terminal-confirmed
local URI list can activate the bridge. Ending the desktop drop does not wait for source disk IO.
A bounded worker captures source identities and lazily opens file handles. It serves
regular files, symlink text, and directory handles over the existing SSH input
transport. Remote requests specify indices and handles, never filesystem paths.
Rendering and terminal input do not perform filesystem reads or source removal.

URI replies carry `X=1:B=1`: the host imports bytes instead of accessing client
paths. Successful Move completion carries a host operation ID in `C`; the local
worker validates the sent manifest, removes the original sources, and returns
`t=L:C=...:o=1` (success) or `o=0` with a failure description. The application
must retain cleanup failures in its operations history. Copy, cancellation,
skipped entries, replaced/modified sources, and incomplete transfers cannot
authorize source removal. Stale transfer generations discard queued file data.

macOS promises, Kitty's cached `dnd-drag-*` files, and clients running on a
machine different from Kitty retain the existing terminal file-transfer path.
To get the local bridge, launch the graphical client locally with its SSH host
option rather than launching the client inside an already remote shell.
