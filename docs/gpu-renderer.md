# Optional GPU scene renderer

The unreleased 0.13 candidate adds hardware scene rendering through Vello 0.11 and wgpu 30. CPU rendering remains the default. This is a local development candidate, with no release tag or consumer pin migration yet.

Enable the `gpu-renderer` Cargo feature. It implies `terminal-graphics`, without enabling media decoding or audio. Select the terminal scene worker with:

```sh
STAR_GRAPHICS_RENDERER=cpu your-app
STAR_GRAPHICS_RENDERER=gpu your-app
STAR_GRAPHICS_RENDERER=auto your-app
```

`gpu` requires a hardware adapter and reports an error if initialization or rendering fails. Software GPU adapters are rejected. `auto` attempts hardware rendering, falls back to CPU if initialization fails, and retries a failed GPU scene on CPU. An unset variable selects CPU; an invalid value is an error. Existing builds without the feature support CPU and explicit Auto fallback.

For reusable offscreen rendering, use `terminal_graphics::renderer::{Backend, SceneRenderer}`. `SceneRenderer::new(Backend::Gpu)` selects hardware; `render(&scene)` returns RGBA pixels. `backend()` reports the requested and selected backend, adapter, graphics API and fallback reason. The reference renderer stays CPU regardless of the environment.

## What runs on the GPU

The shared native renderer records scene fills, paths, strokes, clipping and image composition into Vello. Images are uploaded once while retained in a bounded cache. Layout, text shaping and Swash glyph coverage remain on CPU; glyph masks are composited on GPU. Rendered frames are read back to CPU memory for the existing terminal image transport. This does not provide a zero-copy Kitty surface, accelerate PNG encoding or eliminate SSH bandwidth costs. Existing overlay rendering remains CPU.

The GPU image cache is bounded at 128 MiB and 2,048 entries. Render targets are limited to 8,192 pixels per dimension and 32 million pixels overall. Readback uses a ten-second wait limit. Unsupported paint operations produce errors, allowing Auto to use the CPU path. CPU surface caching is preserved; GPU scenes record their vector operations directly.

Antialiasing and text blending differ slightly from the CPU reference. Hardware output is not pixel-identical; keep CPU for deterministic reference images.

## Reproduce the measurement

From the framework checkout:

```sh
nix develop -c cargo run --release --features gpu-renderer --example gpu-renderer -- target/gpu-proof 1920 1080 40
nix develop -c cargo run --release --features gpu-renderer --example gpu-renderer -- target/gpu-proof-4k 3840 2160 40
# Force adapter unavailability to check Auto fallback.
nix develop -c env WGPU_BACKEND=noop STAR_GPU_BENCH_BACKEND=auto cargo run --release --features gpu-renderer --example gpu-renderer -- target/gpu-fallback 640 360 3
```

The benchmark changes rendered pixels on every frame, includes text, vector paths and scaled images, and writes JSON timings plus CPU/GPU PNGs. Adapter initialization is reported separately. Frame timings include layout, text coverage, rasterization, composition and readback; they exclude benchmark scene construction, PNG encoding, terminal transport and final display. They do not establish interactive end-to-end latency or a frame-rate guarantee.

On the tested AMD Radeon 8060S (RADV STRIX_HALO), Vulkan, 40 changing frames:

| Viewport | CPU median / p95 | GPU median / p95 |
| --- | --- | --- |
| 1920 × 1080 | 7.97 / 8.65 ms | 2.78 / 3.45 ms |
| 3840 × 2160 | 6.78 / 7.20 ms | 6.39 / 7.05 ms |

These are different scene sizes with CPU caching and GPU readback costs; the advantage depends on the workload. Mean absolute channel error against CPU was 0.140/255 at 1080p and 0.070/255 at 4K, with a maximum difference of 50 at antialiased edges. Forced Auto fallback selected CPU with an explicit reason and produced identical pixels.

Linux/Vulkan was exercised on hardware, including two concurrent STAR/MUX Kitty tabs and a real Codex image drop over authenticated loopback SSH. macOS/Metal and other adapters remain unverified. The minimal, CPU graphics and all-feature test matrices pass; all five existing STAR consumers compile against the candidate. See `gpu-renderer-evidence.json` for the recorded results.

Implementation references: [Vello](https://docs.rs/vello/0.11.0/vello/) and [wgpu](https://docs.rs/wgpu/30.0.1/wgpu/).

## Regression: labels disappearing after the first frame

Empty or vector-only live-canvas overlay passes exposed a Vello 0.11 atlas residency issue: its solid-only resolver returned a default atlas description, replacing the persistent texture without invalidating cached image locations. The next frame reused text-mask locations containing no glyph pixels. Every GPU canvas now includes a cached transparent 1×1 image, keeping image-atlas resolution active through those passes.

The hardware regression reproduces the original failure on redraw zero, then checks label ink through 120 redraws with active live canvases and interleaved vector-only scenes:

```sh
nix develop -c cargo test --release --features gpu-renderer gpu_labels_survive -- --ignored --nocapture
```

This test is ignored in the ordinary suite because it requires a hardware adapter; run it explicitly on a GPU machine.

## Terminal typography

The 0.13 candidate includes optional `Capabilities::terminal_typography`. The client queries Kitty’s active PostScript font name, point size and vertical DPI before input ownership passes to the UI. It resolves that face locally, preserves the fractional pixel size and measured terminal cell, and reports a bounded table of face-derived cells for 10–32px zooms. Controllers can resize PTYs and route pointer input using those measurements; remote hosts need no font installation. Missing reports preserve legacy behavior.

A fixed-grid native surface can opt into terminal text with `terminal_font_millipixels`. It keeps fractional physical sizes and shapes styled runs before anchoring glyph clusters to cell columns, retaining ligatures, combining marks, wide text and continuous box-drawing symbols. Editor surfaces without this hint keep the existing per-grapheme grid behavior. Font size hints are bounded to 4–96px. Kitty and KIT still use different glyph rasterizers, so identical font selection does not imply identical antialiasing.

`STAR_GRAPHICS_FONT` and `STAR_GRAPHICS_FONT_SIZE` are explicit frontend overrides. `STAR_GRAPHICS_FONT_DEBUG=1` prints font probe and resolution details. Installed font measurements use the same `FontSystem` loader as rasterization, including disk-backed font files. The regression tests exercise an installed PostScript font, fractional sizes, whole-run shaping and joining glyphs. STAR/MUX’s real-Kitty comparison covers native terminal queries, inherited font cells and reset behavior; it also retains local/SSH input and reconnect acceptance.
