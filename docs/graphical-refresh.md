# Atomic graphical refresh

Native graphical frames retain region deltas, but uploads and presentation are
separate phases. The presenter sends image data with Kitty's transmit-only
action, retaining the old visible frame. Once every changed image is uploaded,
one bounded synchronized-output batch places all replacements before deleting
old images. Large payloads stay outside synchronized-output's safety timeout.

Quiet-window recovery runs every two seconds using the retained PNG encodings.
It restores the entire frame in the same staged transaction. It does not sweep
through visible tiles every 100 ms, rerasterize unchanged content, or reencode
cached images. Transparent regions remain holes for native terminal content.

`scripts/test-atomic-refresh.py` exercises actual Kitty presentation. Generate
its synthetic wire fixture through the flake:

```sh
STAR_KIT_REFRESH_FIXTURE="$PWD/target/atomic-refresh-fixture" \
  cargo test --features terminal-graphics --lib slow_uploads_keep_the_previous_frame_visible_until_the_whole_swap
python3 scripts/test-atomic-refresh.py --kitty PATH --kitten PATH \
  --frames target/atomic-refresh-fixture --output target/atomic-refresh-proof
```

The proof pauses both halfway through upload and after upload completes,
longer than synchronized-output deadlines. Screenshots must show only the old
frame at both stops, and only the replacement after commit. Linux CI retains
the screenshots and JSON report. Renderer tests also cover deltas, resize,
image loss during animation, transparent holes and placement cleanup.

The [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/#display-images-on-screen)
defines separate transmission and placement, including replacement by image and
placement IDs.
