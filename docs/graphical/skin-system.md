# Native skin foundation — first visual acceptance gate

Status: prototype. No application migration or installed-player replacement.

## Ownership

KIT owns nine-slice composition, sprite placement, bitmap-font metrics, bounds,
cache budgets, scale rules, theme-mask operations and interaction geometry.
AMP owns the Classic artwork, reference geometry and player state/actions.
FOLD and other tools are later consumers; no music semantics enter KIT.

## Coordinate contract

Layout uses logical design pixels. UI scale multiplies coordinates uniformly;
window resizing changes flexible widths, not typography or control proportions.
Asset density (1x/2x) is separate from UI scale. Asset slice insets are stated in
source pixels and become logical insets through the asset density. Never scale
an assembled screenshot to fit. Reject undersized nine-slice destinations.

## Phase boundaries

1. Measured specification and offline native player proof.
2. Interactive proof inside Kitty, including pointer states and font comparisons.
3. Bind the AMP-owned player to real state in standalone and embedded views.
4. Linux/Mac visual review at matching viewport, density, palette and data.
5. EQ, Album, Activity, Playlist and dialogs follow individually.
6. Capability-negotiated asset transport, frontend cache and SSH measurements.

The manifest and interactive proof now supply the initial wire contract. Keep
production skin use negotiated; visual acceptance and physical SSH/Mac checks
remain required before describing the design as complete.
The initial proof uses the existing image component and native text compositor.
This demonstrates composition, not the final transport or performance profile.

## Acceptance

Use the approved study's source SVG as the reference, not the combined desktop
screenshot. Export the player at its actual 1352x230 size. Compare like colors,
content and dimensions. Provide exact-size side-by-side, alpha overlay and pixel
difference images. Inspect 1x/2x captures and narrow/normal/wide windows. Test
static button states, rounded/rigid corners, and Unicode body-text fallback.

A numerical image difference is diagnostic: matching blank backgrounds cannot
make incorrect typography or controls pass. Record remaining discrepancies.
Manual Mac Retina/Kitty and Linux/Kitty validation is a separate required gate.

## Immutable artwork and theme layers

`native_surface::skin::assets` provides an owner-scoped `AssetCache`, `Sprite`
and `Layer`. The cache counts encoded plus decoded retained bytes, refuses
replacement under an existing ID, validates PNG dimensions before decoding,
and exposes explicit removal/clear. Borrowed images cannot outlive the cache.
This is local cache policy; it does not claim negotiated network transport.

Sprite metadata includes physical atlas bounds, density, content insets and
optional nine-slice constraints. Compose same-density layers at source size;
then resize using the component's slice metadata. Tint roles affect individual
coverage masks before composition, preserving antialiased edges. Unknown roles,
invalid source rectangles, implicit sprite scaling and mixed densities fail.
Applications own their palette-role mapping and original SVG masters.

## Negotiated skin sprites

A frontend advertises `native_skins` independently of `native_surfaces`. An
application must retain ordinary primitives for older frontends. A surface
may declare `assets`, mapping `skin/<sha256 of PNG bytes>` to a base64 PNG or
`null` for a cached reference. `Primitive::Sprite` names that asset, a physical
atlas source rectangle, and a physical destination rectangle. Its optional
`insets` are left/top/right/bottom source pixels: corners remain unscaled and
the center/edges tile. Without insets, source and destination sizes must match.
An optional tint replaces RGB while retaining mask alpha. Applications select
1x/2x artwork and lay out at that density; finished text or frames are never
implicitly resampled.

The session sends PNGs over the reliable Asset channel once per connection,
then replaces surface payloads with cached references. The frontend hydrates
both newly received scenes and scenes waiting for a late asset, so discarding
an obsolete scene does not lose artwork. Preview-cache churn does not clear
skins. New connections receive assets again. Budgets are 256 declarations and
4 MB of payloads per surface, 512 retained skin IDs / 8 MB encoded / 32 MB
encoded plus anticipated decoded storage per frontend. The native painter
separately bounds decoded originals and resized sprites at 32 MB each.

Tests cover exact corner colors, cached reference repaint, rejected implicit
scaling, wrong content IDs, decoded-size budgets, late-asset hydration, and
one-time reliable delivery. These are protocol and renderer checks; they do
not prove physical SSH latency or macOS visual fidelity.

`Surface::at_density(1..=4)` transforms layout boxes, text sizes, path points,
strokes and hit regions into physical pixels. Source atlas rectangles and
slice insets remain in their already-selected physical source density. The
result is validated before use; overflows and oversized surfaces return errors.
