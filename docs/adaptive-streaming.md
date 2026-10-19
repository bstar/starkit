# Experimental adaptive HLS

The `streaming` feature retains original-file HTTP ranges and adds token-protected
`/v1/hls/{opaque-id}/master.m3u8`. Catalogs advertise `streaming.hls = true`.
No public Rust signatures change. FFmpeg/ffprobe are external supervised tools;
this feature does not link codec or audio-device libraries.

The Radeon 8060S driver produced corrupt HEVC pictures in both bitrate-control
and constant-quantization modes. Some corrupt fragments decode without any error
message. Linux therefore defaults to CPU HEVC (libx265), preserving the 4K ladder.
VAAPI is opt-in with `STAR_STREAM_ENCODER=vaapi` for driver testing; its successful
startup probe does not establish picture correctness. Integration checks compare
decoded pixels against the source as well as rejecting decoder errors.

The master offers renditions only up to the source dimensions. A real encoder
probe chooses CPU HEVC on Linux or VideoToolbox on Darwin; software H.264
fallback caps at 1080p when HEVC is unavailable. HEVC offers up to 4K. Bitrates are 1.2, 2.8, 6
and 20 Mbps plus AAC stereo. HDR is currently tone-mapped to SDR. No subtitle
tracks are generated. Original-file delivery remains unchanged.

Seekable VOD playlists describe the complete timeline. Approximately six-second
chunks are encoded only on request, so seeks do not require encoding the movie
from the beginning or keeping every quality running. Every independent chunk
has its own MP4 initialization section and explicit HLS discontinuity; rendition
boundaries share the source-frame-aligned interval. Input paths come only from
the server's canonicalized resource allowlist. URL paths never accept host paths.

At most two movie contexts exist per SSH service, with one encode at a time per
movie and 384 MiB of disk cache per movie. Least-recently-used fragments are
removed first. Individual fragments are limited to 48 MiB. Metadata/encoder
commands have time and output bounds; stopping the service kills and waits for
its codec subprocesses. Cache files are anonymous temporary ownership objects
and are removed on normal shutdown. File handles preserve in-flight responses
when a cache entry is evicted.

FFmpeg defaults to PATH. `STAR_STREAM_FFMPEG` and `STAR_STREAM_FFPROBE` can select
absolute binaries; `STAR_STREAM_ENCODER=software` selects the H.264 CPU fallback for
testing. SCREEN's experimental Linux launcher uses the system FFmpeg by default
so VAAPI uses the host's installed driver stack rather than mismatched Nix GPU
libraries. Application-linked codecs retain their normal Nix runtime.

Build the `streaming-probe` example with `--release --no-default-features
--features streaming --example streaming-probe`, then run `python3
scripts/test-hls.py`. It generates a temporary MKV with FLAC audio and decodes
first, next and final segments from multiple renditions, checks authentication
and byte ranges, and removes its fixtures. The standalone tvOS project's host
proof additionally tests native HLS down/up selection under throttled delivery.
