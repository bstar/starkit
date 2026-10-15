# STAR native chrome

Original STAR/AMP Classic Rack panel artwork, provided by STAR/KIT under its
MIT license. No proprietary Winamp artwork or fonts are included.

The editable panel, rigid panel and inset well masters live in `source/`.
Their original coverage masks retain 1x and 2x physical density. `manifest.json`
describes layer order, palette roles and fixed-corner nine-slice geometry.
`src/native_surface/style/artwork.rs` bundles those masks into the framework;
applications do not need runtime asset directories or a copy of AMP.

The default palette preserves the original slate chassis and green accent.
Theme palettes tint each role, while frame corners retain their source pixels.
Musical displays, album covers and AMP-specific glyphs remain in STAR/AMP.
