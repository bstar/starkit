# Pictures in a terminal

`starkit::graphics` is the part of this crate that is not really about drawing.
Putting an image on a terminal is four decisions — can this terminal do it,
how big is a cell, what has already been encoded, and what happens when the
answer is no — and only the last of them is a drawing problem.

## Probe before raw mode

Asking a terminal what it can do means writing an escape sequence and reading
the reply off stdin. That works exactly once per run, and only before the
application owns the keyboard:

```rust
let graphics = Graphics::probe_if_tty(Mode::parse(&cfg.ui.graphics));  // first
let mut term = starkit::term::init()?;                                 // then
```

Afterwards, the reply arrives interleaved with whatever the user is typing:
the probe reports their keystrokes as the terminal's answer, and the keys they
meant to press are eaten. The ordering cannot be expressed in a type across a
crate boundary, so `term::init` records that it ran and `Graphics::probe`
asserts against it — a debug build panics, a release build degrades to
`Graphics::disabled()` with a warning.

`probe_if_tty` also refuses when stdout or stdin is not a terminal. On Unix,
the query owns a separate nonblocking `/dev/tty` descriptor, reads synchronously,
and restores the original terminal attributes before returning. No reader
thread can outlive the deadline or restore cooked mode after the UI starts.
It stops at the final device-status response, leaving subsequent keyboard input
queued for the application. Reads are capped at 8,192 bytes.

The default deadline is two seconds; terminals that answer promptly return
immediately. `STAR_GRAPHICS_PROBE_TIMEOUT_MS` accepts 50–10,000 milliseconds
for a particularly slow connection or an automated fixture. Automatic mode
queries terminals with graphics hints and SSH sessions; inside tmux or screen
it keeps the cell fallback. Explicit `kitty` mode can query configured
multiplexer passthrough. Unsupported native platforms use the cell fallback.

A positive cell-size response or a valid pixel window measurement establishes
measured geometry. The image picker's default 10×20 cell estimate is not
reported as a measurement and cannot enable pixel mouse input.

## Modes

`[ui] graphics` is `auto`, `kitty`, `blocks` or `off`, and it is about the
application's *photographs* — covers, avatars, attachments.

| Mode | Photographs | Rasterised icons |
|---|---|---|
| `auto` | whatever the probe found | yes, where the probe found one |
| `kitty` | kitty, believed rather than detected | kitty |
| `blocks` | half blocks | yes, where the probe found one |
| `off` | nothing | yes, where the probe found one |

`kitty` exists for the case the probe cannot see: over ssh and inside a
multiplexer the outer terminal does not answer, and being told is better than
guessing. Turning photographs off is not a statement about an application's own
icons, which is why `Graphics::raster` keeps working in `blocks` and `off`.

## Cache keys

`Graphics` holds one `ImageCache`, shared by everything it encodes, keyed by
three things:

```rust
Key { id: ImageId, cells: Size, pixels: (u32, u32) }
```

**`id` is the picture, not a name for it.** Keying a cover on its track looked
equivalent and was not: cycling to another of an album's covers leaves the
track, the panel and the rectangle exactly as they were, so every cover after
the first was drawn as the one before it while the caption underneath said
otherwise. `ImageId::of_arc` is the address of the decoded image, and the cache
holds the `Arc` for as long as it holds the protocol — a freed address comes
straight back out of the allocator, and without that the next picture along
inherits this one's entry. `ImageId::of` hashes a recipe instead, for a picture
the application draws itself and whose identity is *what it is about to draw*:
which icon, in which colours.

**Cells and pixels are two different facts.** A protocol is transmitted as
pixels and *placed* over a number of cells, and the same pixel size can be
reached from different cell counts — four cells of seven pixels and two cells
of fourteen are both twenty-eight. Keyed on pixels alone, a protocol built for
two cells is handed to a picture spanning four, and the two cells it does not
cover keep whatever the terminal had there before: a graphics placement is not
erased by painting the cell, only by another placement or a delete.

The cache is least-recently-used, because the thing on the other side of it is
the terminal's own memory. `forget(id)` releases one picture, `forget_all()`
everything, and `forget_unused(&drawn)` everything a frame did not put on
screen — which is how a scrolling list of images stays bounded without any one
place knowing when an image left the viewport.

## Drawing

```rust
if let Some(p) = graphics.protocol(id, &img, rect) {
    ratatui_image::Image::new(p).render(rect, buf);
} else {
    graphics::halfblocks(&img, rect, buf);
}
```

`None` always means "draw it yourself". There is no error to report: a terminal
without a protocol, an encoder that failed and a mode of `off` all want the
same thing from the caller, and half blocks are a worse picture rather than no
picture. `placeholder` (`░`) is for the rectangle whose bytes have not arrived
yet, so the layout does not move when they do.

Two rules that are not visible from the signatures:

- **Clipping is per protocol.** Kitty tolerates a placement running off the
  edge of the screen. Sixel and iTerm2 do not — they draw from the top left of
  where they land, whatever was asked for. A rectangle the viewport cuts in
  half is drawn as half blocks for the rows that survive.
- **A one-cell placement needs mending.** The kitty renderer writes a whole
  image row into its first cell and ends it by moving right by the width less
  one and down by the height less one. For a single cell both moves are zero,
  and a zero parameter to those controls means one — so the cursor ends a row
  too low and ratatui prints the rest of the line there. `mend_unit_placeholder`
  rewrites that exact tail; call it after rendering any 1x1 image.

## Rasterising

`graphics::raster` fills polygons into pixels, and knows nothing about what
they are. A caller keeps its shapes on whatever grid it likes — STAR/AMP's
transport icons are the Material Design ones on their 24-unit grid — hands over
a mapping from pixel coordinates onto that grid, and gets anti-aliasing from
supersampling the same shape test that decides the fill. There is no separate
outline pass, so a shape and its edge cannot disagree.

This exists because a terminal program cannot ship a font. A Nerd Font icon is
drawn by whatever typeface the terminal fell back to, at that face's size and
weight, and two machines set to different fonts draw two different rows of
buttons from the same bytes. An icon rasterised at the cell size and sent over
the graphics protocol is the same on all of them.

The images are opaque on purpose. What a graphics protocol shows behind a
transparent pixel is the cell's background, which is whatever style the
placeholder cell happened to keep, and painting the background colour in is the
only way to be sure of it.

## Decoding

Every image either application decodes came from outside it. `decode_limited`
and `open_limited` check the declared dimensions against the header before any
pixels are read: the `image` crate's own defaults allow any width and height
and cap only the total allocation, at 512 MiB, so a file of a few kilobytes
claiming to be 10000x10000 becomes four hundred megabytes before anything
notices it is absurd — on a worker that a track change is waiting for.
