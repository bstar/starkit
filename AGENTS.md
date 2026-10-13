# Working notes

Context that is not derivable from the code or the history. It lives in the
repository rather than in any one machine's notes because this is developed on
both Linux and macOS and the repository is the only thing every session sees.

This is the only notes file of its kind here. Do not add a second one beside
it under another name; everything that reads such a file reads `AGENTS.md`.

## Public framework and four reference consumers

STAR/KIT is being prepared as a public framework for native terminal apps.
STAR/AMP, STAR/FOLD, STAR/CORD and STAR/WIRE are its reference consumers.
Keep cell and graphical presentations, SSH capability negotiation, bounded
media transport, visual fidelity and performance in shared engine code.
Application domain models and product rules remain in their own repositories.

Public readiness is a phased program. Alignment comes first, then responsive
panes and shared interaction, consumer migrations, examples and stability gates.
Do not describe APIs or platform behavior as mature without evidence.

Release through `CHANGELOG.md`, the Cargo version and an immutable `vX.Y.Z`
tag. Each application takes the tag and regenerated git-sourced Cargo.lock in
a separate dependency commit. Never commit a path-patched lockfile. An API
change requires a minor version on 0.x; compatible fixes use a patch version.

Before changing a signature, read all four consumers. The active AMP checkout
on the Linux workstation is `../staramp-graphical`; the other checkouts are
`../starfold`, `../starcord` and `../starwire`. Consumer CI tests disposable
copies against the candidate KIT checkout using `scripts/check-consumer.py`.

## Framework work program

The user authorized completing the framework plan autonomously. Preserve
uncommitted application work and keep release pins portable throughout.

- [x] Align four apps on a published baseline; restore FOLD graphics and cell dragging.
- [x] Four-consumer compatibility CI and a benchmark that changes rendered pixels.
- [ ] Responsive panes, focus, visibility, wrapping and stable capture; migrate FOLD.
- [ ] Migrate AMP, CORD and WIRE presentation geometry without moving domain models.
- [ ] Shared media widgets and remove FOLD's AMP dependency for generic video controls.
- [ ] Capability profiles for ASCII/cell/native presentation and explicit fallbacks.
- [ ] Separate optional graphics/media dependencies and bound media cache resources.
- [ ] Agent diagnostics/replay, external starter, feature/visual/performance gates,
      packaging and public preview documentation. Claim maturity only for proven paths.

## Working on it from an application

A released change reaches an application through its pinned tag, which is the
cost of pinning tags and is worth paying. To try one before it is, in any of
the four:

```toml
# ../staramp/.cargo/config.toml -- untracked, and in .gitignore
[patch."https://github.com/bstar/starkit"]
starkit = { path = "../starkit" }
```

`.cargo/` is ignored in all four applications rather than merely left
untracked: a committed one points their CI at a path that does not exist on
the runner, and the failure it produces names cargo rather than the file.
Delete it when the change is tagged and the application has moved to the new
tag. Never commit a `Cargo.lock` written while the patch was in place --
starfold's own `AGENTS.md` tells of the one time this happened, and says a
`Cargo.lock` changed with no dependency-version bump to explain it is the
tell to check for first; the same applies wherever the patch is used, so
watch for it in all four.

## Responsive layout adoption

`panes` is the new responsive API; `dock` remains compatible. FOLD is the first
pane consumer; the remaining app layouts are being migrated incrementally: one geometry must drive rendering, hit testing, capture,
wrapping and visibility. Preserve manual sizing and distinguish user hiding
from automatic collapse. Verify small windows and cell fallbacks alongside
native pixels before replacing an application's layout.

## Probe before raw mode

`Graphics::probe` writes capability queries to the terminal and reads the
replies back off stdin. It has to run **before** `term::init` enables raw mode
and the alternate screen, and the late replies that arrive after it returns are
discarded by `drain_stdin`. Probing afterwards reads the user's keystrokes as
capability replies and reports whatever they typed as the terminal's answer.

The ordering is not expressible in the type system across a crate boundary, so
`term::init` records that it ran and `probe` debug-asserts against it: a debug
build panics, and a release build degrades to `Graphics::disabled()` with a
warning rather than corrupting the session.

## Themes are shared assets

The sixteen theme files here are read by all four applications. The core resolver
deserialises the tables it knows -- `meta`, `base16`, `app`, `chrome`, `panel`,
`row`, `status` -- and keeps everything else in `ThemeFile::extra`, so a file
may carry `[vis]` for STAR/AMP's analyzer and `[chat]` for STAR/CORD's message
list at the same time and neither one sees the other's table as an error.

That is a property to preserve. A resolver that rejects unknown tables, or a
schema that flattens the app tables into the core struct, makes every theme
file the property of one application.

Resolution is pinned at both ends: `testdata/golden/` here, and STAR/AMP's own
`testdata/theme-golden/`, record what each built-in resolves to role by role. A
derivation change that is deliberate is a diff of colours in those directories.
One that is not is a failing test.

## Chrome is shared

From 0.3, a panel or overlay is expected to be built out of what is here
rather than reimplemented against `ratatui::widgets::Block` directly: a
docked panel or a full-screen dialog goes through `chrome::frame` or
`chrome::overlay`, a list's scroll position through `chrome::scrollbar`, and
a yes/no question through `chrome::confirm`.
This is what `chrome::frame` existing at all bought back from starcord and
starfold's byte-identical `panels::frame` and staramp's nine hand-drawn
variants: a panel that builds its own `Block` instead is not a variation on
the look, it is a fork of it, and the next theming or border-colour change
will not reach it.

## `-A dead_code`, and why

CI runs `cargo clippy --all-targets --all-features -- -D warnings -A
dead_code`. `dead_code` is the one lint that is allowed, because a library
module lands here complete and tested one work package before the application
that will call it, and `--all-features` means the build always contains code
that only some of the four consumers ever reaches. Denying it would mean either
`#[allow]` scattered through the crate or writing the caller first. Every other
lint is an error.

## Building

Through the flake, always, on every machine:

```sh
nix develop -c cargo test --all-features
nix develop -c cargo test --no-default-features
```

Both ends of the feature matrix, because a feature only one application turns
on is exactly the one that rots. `nix flake check` builds the package, which
runs the tests with every feature on.

## Experimental native terminal pixels

On `experiment/terminal-graphics`, the optional renderer is a Rust thread using
cosmic-text/Swash and tiny-skia. Native RGBA frames pass directly to the Kitty
region presenter. No browser, JavaScript runtime or display server is used.
Bundled Liberation fonts and their OFL notice live in `assets/fonts`; installed
fonts supply Unicode fallback. Keep the native rendering, protocol and font
assets in KIT so applications share one implementation. The ordinary feature
matrix remains unaffected.

Native embedded surfaces use `native_surface::{Surface, Primitive, PixelRect}`.
The contract is available without the renderer feature so a helper can own its
UI without linking a rasterizer. Pixel coordinates are local, bounded and clipped
to the containing surface; opaque hit actions remain the helper's responsibility.
`native_surfaces` is an additive negotiated frontend capability. Terminal input
defaults to cell precision. The Kitty frontend can enable SGR 1016 only with
measured pixel geometry, outside multiplexers. `pixel_mouse` reports that enabled
mode separately from the legacy cell-precision enum; graphics support alone must
not claim pixel pointer precision. Restore 1016 on normal exit and panic.

Pixel root layout uses `terminal_graphics::placement`: bounded logical regions
mapped to pixel destinations, with shared column/split gaps and optional panel
insets. Pointer projection and raster row edges must stay identical. Preserve
terminal-derived font sizes when placing regions; do not resize text bitmaps.
Native surfaces carry physical text sizes, so resizing their host changes their
layout boxes without scaling the glyphs. `pixel_layout` is separately negotiated.


## Native video preview experiment

`media` owns native FFmpeg poster/proxy/decoding and CPAL output. Terminal
`media` messages negotiate video and trusted local file access separately; a
remote frontend must never open a host-supplied local path. Generations scope
chunks, credits and status. Session output prioritizes control and scenes over
media. Keep cancellation out of UI locks and audio callbacks free of allocation.

FFmpeg 9 filter sink setters can silently fail to constrain channel layouts.
The AAC proxy uses an explicit `aformat` filter and checks format/rate/channels
before encoding; passing a mono plane to a stereo AAC encoder caused a confirmed
segfault during development. Keep that guard and the mono-source integration test.

Muxer writes are often small TS packets, not 32 KiB buffers. Aggregate them
before charging the byte credit window; otherwise eight small packets can fill
the bounded decoder channel while unused byte credits keep the sender running.
`StreamIo` flushes the final partial chunk when its output context is released.

## SSH audio relay

Standalone AMP uses the additive `audio_pcm_v2` capability and `Pcm*` messages.
`pcm` validates bounded source-format blocks without a device; `pcm_output`
owns native format selection and optional float64 conversion. `audio_v2` owns
the frontend worker, eight credits, epoch cancellation, callback progress and
end-of-stream drain. Negotiated `audio_cache_v1` returns credits after PCM is
written to an anonymous whole-track disk cache; legacy PCM returns them after
conversion enters the playback ring. Cached pause preserves the file. Standalone
connection workers receive, refill and play PCM independently of UI presentation.
Received frame reports drive the buffer bar; audible completion drives the queue.
Block format and processing provenance may differ from
the original source spec without reopening the output. Preserve the legacy
fixed-format relay for FOLD/embed clients. Never silently convert a strict PCM
route or fall back to server audio on a device error.

`terminal_graphics::audio` plays negotiated `audio_relay` PCM on the frontend.
Its separate opaque session and epoch never carry host paths. Blocks are
bounded 48 kHz stereo i16, converted by FFmpeg to the local device format on a
worker. CPAL's callback only drains a ring. Eight block credits bound transport;
credits return after conversion enters the bounded playback ring. New epochs,
route close and disconnect cancel and join the worker. A failed local device
reports an error to the host; it must not silently switch to host playback.
