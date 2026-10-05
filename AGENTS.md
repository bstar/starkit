# Working notes

Context that is not derivable from the code or the history. It lives in the
repository rather than in any one machine's notes because this is developed on
both Linux and macOS and the repository is the only thing every session sees.

This is the only notes file of its kind here. Do not add a second one beside
it under another name; everything that reads such a file reads `AGENTS.md`.

## Released by tag, consumed by three applications

This crate was lifted out of STAR/AMP a module at a time and the move is
finished: the leaf modules, the directory rule, file logging, the theme
engine, the terminal graphics, the panel chrome, the key table and help
overlay and the HTTP defaults came from there, and the layout engine, text
field, wrapper and virtual list were written here for STAR/CORD. STAR/FOLD, a
file manager on the same foundation, joined as a third consumer for 0.3.
`CHANGELOG.md` says what is here.

It is not published to crates.io -- there is no fourth consumer to publish it
for -- so all three applications depend on a **git tag**, and nothing that
lands here reaches any of them until one is cut. Releasing is
`CHANGELOG.md`, the version in `Cargo.toml`, a `vX.Y.Z` tag, and then a
commit in each application that moves its `tag` and its `Cargo.lock` and
does nothing else ("Take starkit 0.Y"). Semver 0.x: an API change is a minor
bump, a fix is a patch.

## Three consumers, all of them known

Every public item in this crate exists because STAR/AMP, STAR/CORD or
STAR/FOLD needed it, and most of them are called from more than one. There is
no fourth caller and no published API to keep compatible with a stranger,
which is the one real advantage this arrangement has: **before changing a
signature, read all three call sites.** They are checked out beside this
repository:

```sh
grep -rn "starkit::" ../staramp/src ../starcord/src ../starfold/src
```

A change that is awkward at one of them and impossible at another is not a
refactor, it is a new function. Adding one is cheaper than bending a shared
one until an application stops liking it.

This is also why the crate is not documented as a general framework. Its
README says so out loud, and a feature request with no consumer in any of the
three applications is a fork, not an issue.

## Working on it from an application

A change here is invisible to an application until it is tagged, which is the
cost of pinning tags and is worth paying. To try one before it is, in any of
the three:

```toml
# ../staramp/.cargo/config.toml -- untracked, and in .gitignore
[patch."https://github.com/bstar/starkit"]
starkit = { path = "../starkit" }
```

`.cargo/` is ignored in all three applications rather than merely left
untracked: a committed one points their CI at a path that does not exist on
the runner, and the failure it produces names cargo rather than the file.
Delete it when the change is tagged and the application has moved to the new
tag. Never commit a `Cargo.lock` written while the patch was in place --
starfold's own `AGENTS.md` tells of the one time this happened, and says a
`Cargo.lock` changed with no dependency-version bump to explain it is the
tell to check for first; the same applies wherever the patch is used, so
watch for it in all three.

## `dock` has no consumer yet

The dock layout engine (`dock`) is used by none of the three applications.
Each keeps its own hand-rolled one-column layout instead -- STAR/CORD's and
STAR/FOLD's each in a `ui/layout.rs` that says so in the application's own
`AGENTS.md`, staramp's inline in `ui/app.rs` -- because none of them has more
than one arrangement of panels to choose between, and a single column needs
no seam to drag. It is kept rather than removed: the tests pin real,
exercised behaviour even with nothing calling it in an application, and the
trigger to adopt it anywhere is a consumer that actually wants draggable
seams, not a tidying pass here.

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

The sixteen theme files here are read by all three applications. The core resolver
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
that only some of the three consumers ever reaches. Denying it would mean either
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
is still cell precision; do not advertise pixel pointer precision.

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
