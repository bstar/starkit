# Default native style

`native_surface::style::Style` is KIT's default graphical visual language,
extracted from STAR/AMP's Classic Rack. It owns the palette, compact physical
text sizes, original 1x/2x panel artwork, title strips, inset wells, centered
controls, local hover fills, selection rows, meters and timeline/ledger marks.
It has no player, recorder, filesystem configuration or application actions.
STAR/AMP and STAR/REEL consume it; the independent starter demonstrates it.

A minimal panel needs no asset loader or private color constants:

```rust
use starkit::native_surface::{style::Style, PixelRect};

let style = Style::default();
let mut panel = style.panel(360, 220, "CAPTURE");
style.label(&mut panel, PixelRect::new(12, 42, 336, 24), "Choose an input");
style.button(&mut panel, PixelRect::new(12, 78, 160, 28), "Microphone", "mic", false);
style.primary_button(&mut panel, PixelRect::new(12, 120, 160, 28), "Record", "record", false);
```

Action strings remain opaque. The application interprets `panel.hits`; KIT's
frontend paints hover fills without changing controller state. Button text uses
measured shaping and real centering, including Unicode and clipped long labels.
The pointer target retains its full requested rectangle around the compact face.
A meter has no pointer target. `row` retains checked state separately from focus.

With `terminal-graphics`, `style.scene(viewport)` creates a matching scene and
`style.mount(&mut scene, panel, x, y)?` installs one physical placement for pixels,
clipping and pointer projection. It validates bounds and keeps surfaces after a
Menu/Dialog marker in the overlay. The starter uses these public methods outside
KIT's workspace. `Palette` is now a generic style type; `media_controls::Palette`
remains a compatible re-export with the same serialized fields.

Negotiate artwork with `style.with_native_skins(capabilities.native_skins)`.
The primitive fallback keeps the same palette and layout without image assets.
The palette and controls also compile without default features; graphics-only
builds do not link FFmpeg or audio devices. This native style does not replace
cell theme resolution or force existing applications to discard their themes.
`Style::new(Palette::from_theme(&theme))` uses existing KIT theme roles, and
`metrics` exposes the shared padding, gap, title/control heights and text sizes.
The default construction uses Classic's original reference palette.

`frame_at_density` selects the original 1x/2x bitmap corners. When drawing a 2x
composition, scale the layout, text and hit regions together and select the 2x
source before presentation; do not enlarge a finished UI image. Low-level
`Surface` and `classic` primitives remain available for custom content. AMP's
musical artwork and specialized Silver treatment remain its domain adapter.

This is part of the untagged 0.9 candidate. Development applications pin an
immutable candidate revision and build without a sibling KIT checkout. Release
tags remain separate. To check newer local changes without rewriting a
consumer's git-sourced lock, use its candidate wrapper:

```sh
CARGO_TARGET_DIR="$PWD/target" bash scripts/check-kit.sh --cargo build --release --offline
```

Use the shared [theme picker](themes.md#shared-picker) for Alt+T theme selection;
`Style::from_named` resolves the same registry for native chrome.

## Dense workspaces

KIT also provides quiet workspace composition through `workspace_panel`, `tool`,
`property`, `section`, `numeric` and `time_ruler`. These use `Style.workspace`
palette roles: neutral panels, shallow control fills and fine separators. They
retain the selected theme and keep accent colors for active modes and selection.
Classic's artwork remains available for compact instruments and media players.

`workspace_panel` opts into `Surface::with_ui_text()`: bundled Liberation Sans
for UI labels and Liberation Mono for numeric text, in physical pixels. Legacy
surfaces keep the configured terminal face. The additive `ui_font` hint is
ignored by older frontends, and the font choice participates in the glyph cache.
A property field keeps its label, value, hover and hit target aligned; read-only
fields have no action. No app-specific RGB tables or font loading are required.

`time_ruler` takes `editor_controls::TimeView`, the same transform used for clips
and seeking. It chooses readable frame intervals at the current zoom.
`editor_controls::timecode` formats the project's non-drop-frame HH:MM:SS:FF.
REEL owns fitting and manual zoom, track names, clip edits and recording settings.

`workspace::RECORDING_RED` is a theme-independent capture-state color. Use
`Style::recording_badge` for a passive high-contrast REC/elapsed indicator.
The application owns capture state and keeps these guides outside saved media.

Use `Style::modal_panel` for dialogs: quiet workspace chrome with a continuous
two-pixel theme-accent focus border. KIT’s theme picker and REEL’s audio and
text dialogs share it.

`Style::keycaps("Ctrl + Shift + S", scale, max_width)` creates a compact,
noninteractive key display using shared face, shadow and typography roles. Mount
only that small surface over a live canvas; it has no pointer targets. REEL also
uses its drawing primitives for video effects, keeping capture policy in REEL.

Use `Surface::retain_nodes` when folding or filtering a native surface. It remaps
centered labels, text decoration rules and hover references and removes metadata
for discarded nodes. Filter hit regions separately by their visible geometry.

## Display lifetime and recovery

The framebuffer is authoritative; terminal image placements are disposable.
`RenderMessage::Frame` carries the exact scene used to rasterize it, including
local hover changes that can share a controller revision. The frontend uses that
snapshot for live-canvas holes, video, overlays and hit geometry. A newer scene
cannot reposition a live layer over an older frame while rasterization is pending.

KIT restores cached artwork on focus return. Ctrl+L repaints all graphical layers
without replacing the native renderer, clearing its asset cache or restarting the
application. Embedded keyboard capture retains its application's Ctrl+L shortcut.
Static live canvases and paused video retain their pixels for restoration without
a new producer frame.

Attached sessions with active live streams sample producers on an 8 ms timer,
independently of scene rebuilding. This avoids imposing a 30 Hz controller clock
on a 60 Hz compositor during direct manipulation. Apps without live producers,
closed streams and detached sessions retain the quiet 33 ms polling interval.

Quiet regions are also refreshed in a bounded round-robin pass. Cached encoded
tiles avoid rerasterizing/reencoding the UI; replacements are placed before old
images are retired. The encoded region cache is capped at 15 MB. The pass targets
two seconds, with actual completion depending on terminal throughput and loop
latency. This repairs unexpected terminal placement loss even while another panel
keeps animating. Focus return requests an immediate full repaint instead.

`scripts/test-live-canvas.py` deliberately erases a private Kitty display and
checks that static controls recover alongside two active live canvases. It also
checks Ctrl+L, resize, frontend loss and reattachment. For application checks:

```sh
python3 scripts/test-display-recovery.py --kitty KITTY --kitten KITTEN \
  --app amp --binary ~/.local/bin/staramp --output /tmp/amp-recovery
python3 scripts/test-display-recovery.py --kitty KITTY --kitten KITTEN \
  --app reel --binary ~/.local/bin/starreel --output /tmp/reel-recovery
```

The tests use private configurations, runtimes and projects. REEL's optional
`--project FIXTURE --edit` copies a fixture and tests paused timeline video.
Results and actual terminal screenshots are retained in the output directory.
Local Linux/Kitty checks cover AMP, REEL capture and paused playback; other
terminal implementations and physical macOS behavior still need verification.
