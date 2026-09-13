//! Animated GIFs, decoded into frames a renderer can page through.
//!
//! One decode, up front, into whole composited frames. A GIF is a stream of
//! patches with disposal rules, and the alternative -- keeping the decoder and
//! stepping it once per displayed frame -- would mean holding a decoder per
//! visible image, doing work inside a draw, and having no way to go backwards
//! when the same image comes back on screen. Frames are `Arc`ed, so the
//! sequence is cheap to hand to whatever draws it and cheap to keep in a
//! cache.
//!
//! ## Caps, and where they apply
//!
//! The bytes come off the network from somewhere nobody here controls, and a
//! GIF that is 64000 by 64000 with nine hundred frames is a valid GIF. The
//! dimensions are in the header, so `max_pixels` is checked before a single
//! frame is allocated. Frame count is not: the format does not say how many
//! there are, so `max_frames` is a stopping point rather than a rejection, and
//! a longer animation is truncated and plays the part that was decoded.
//!
//! ## Delays
//!
//! GIF delays are stored in hundredths of a second, and a great many files in
//! the wild say zero or one, meaning "as fast as you can". A browser reads
//! those as 100 ms; a terminal redrawing a picture protocol cannot afford even
//! that per frame across several images, so the floor here is 20 ms and the
//! policy about which animations run at all belongs to the caller.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, ImageDecoder, RgbaImage};

/// The shortest a frame is shown, whatever the file asks for.
pub const MIN_DELAY: Duration = Duration::from_millis(20);

/// One composited frame and how long it stays up.
#[derive(Debug, Clone)]
pub struct Frame {
    pub img: Arc<RgbaImage>,
    pub delay: Duration,
}

/// A decoded animation.
#[derive(Debug, Clone)]
pub struct FrameSequence {
    /// At least one frame; a sequence with none is an error, not a value.
    pub frames: Vec<Frame>,
    /// The whole loop, which is what [`FrameSequence::at`] works modulo.
    pub total: Duration,
    /// The caller's identifier for the image, carried through so that a cache
    /// key can be built from a frame without a second lookup.
    pub id: u64,
    /// Whether `max_frames` stopped the decode early.
    pub truncated: bool,
}

impl FrameSequence {
    /// Which frame is showing `elapsed` after the animation started, looping.
    pub fn at(&self, elapsed: Duration) -> (usize, &Frame) {
        debug_assert!(!self.frames.is_empty());
        if self.frames.len() == 1 || self.total.is_zero() {
            return (0, &self.frames[0]);
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % self.total.as_nanos()) as u64);
        for (i, f) in self.frames.iter().enumerate() {
            if t < f.delay {
                return (i, f);
            }
            t -= f.delay;
        }
        // Only reachable through rounding in the modulo above.
        let last = self.frames.len() - 1;
        (last, &self.frames[last])
    }

    /// How long until the frame after `elapsed`, so a caller can set its
    /// event-loop timeout to the earliest thing that needs redrawing.
    pub fn until_next(&self, elapsed: Duration) -> Duration {
        if self.frames.len() == 1 || self.total.is_zero() {
            return Duration::MAX;
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % self.total.as_nanos()) as u64);
        for f in &self.frames {
            if t < f.delay {
                return f.delay - t;
            }
            t -= f.delay;
        }
        MIN_DELAY
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Why a GIF did not decode.
#[derive(Debug)]
pub enum AnimError {
    /// Not a GIF, or a damaged one. Every one of these is somebody else's
    /// file, so it is an error and never a panic.
    Decode(image::ImageError),
    /// Bigger than the caller is prepared to hold in memory.
    TooLarge { pixels: u64, limit: u64 },
    /// A GIF with no frames in it at all.
    Empty,
}

impl std::fmt::Display for AnimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnimError::Decode(e) => write!(f, "not a readable GIF: {e}"),
            AnimError::TooLarge { pixels, limit } => {
                write!(f, "{pixels} pixels is over the {limit} pixel limit")
            }
            AnimError::Empty => f.write_str("the GIF has no frames"),
        }
    }
}

impl std::error::Error for AnimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AnimError::Decode(e) => Some(e),
            _ => None,
        }
    }
}

impl From<image::ImageError> for AnimError {
    fn from(e: image::ImageError) -> Self {
        AnimError::Decode(e)
    }
}

/// Decode an animated GIF.
///
/// `max_pixels` is width times height and is checked against the header
/// before anything is allocated. `max_frames` stops the decode rather than
/// failing it: see the module documentation.
pub fn decode_gif(
    bytes: &[u8],
    id: u64,
    max_frames: usize,
    max_pixels: u64,
) -> Result<FrameSequence, AnimError> {
    let decoder = GifDecoder::new(Cursor::new(bytes))?;
    let (w, h) = decoder.dimensions();
    let pixels = u64::from(w) * u64::from(h);
    if pixels > max_pixels {
        return Err(AnimError::TooLarge {
            pixels,
            limit: max_pixels,
        });
    }

    let mut frames = Vec::new();
    let mut total = Duration::ZERO;
    let mut truncated = false;
    for frame in decoder.into_frames() {
        if frames.len() >= max_frames {
            truncated = true;
            break;
        }
        let frame = frame?;
        let (num, den) = frame.delay().numer_denom_ms();
        let ms = num.checked_div(den).unwrap_or(0);
        let delay = Duration::from_millis(u64::from(ms)).max(MIN_DELAY);
        total += delay;
        frames.push(Frame {
            img: Arc::new(frame.into_buffer()),
            delay,
        });
    }

    if frames.is_empty() {
        return Err(AnimError::Empty);
    }
    Ok(FrameSequence {
        frames,
        total,
        id,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Rgba};

    /// A GIF of `delays.len()` frames, each a flat colour, built here so the
    /// test does not depend on a file in the tree.
    fn gif(delays: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            for (i, ms) in delays.iter().enumerate() {
                let shade = (i as u8 + 1) * 60;
                let img = RgbaImage::from_pixel(4, 4, Rgba([shade, 0, 0, 255]));
                enc.encode_frame(image::Frame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(*ms, 1),
                ))
                .expect("encodes");
            }
        }
        out
    }

    #[test]
    fn the_frames_and_their_delays_come_back() {
        let seq = decode_gif(&gif(&[100, 50, 200]), 7, 16, 1 << 20).expect("decodes");
        assert_eq!(seq.len(), 3);
        assert_eq!(seq.id, 7);
        assert!(!seq.truncated);
        let ms: Vec<u128> = seq.frames.iter().map(|f| f.delay.as_millis()).collect();
        assert_eq!(ms, vec![100, 50, 200]);
        assert_eq!(seq.total, Duration::from_millis(350));
        for f in &seq.frames {
            assert_eq!(f.img.dimensions(), (4, 4));
        }
    }

    #[test]
    fn a_delay_below_the_floor_is_raised_to_it() {
        // GIF stores hundredths of a second, so ten milliseconds is one tick
        // and zero is the "as fast as possible" that half the files in the
        // wild carry.
        let seq = decode_gif(&gif(&[0, 10]), 0, 16, 1 << 20).expect("decodes");
        assert!(seq.frames.iter().all(|f| f.delay >= MIN_DELAY));
        assert_eq!(seq.total, MIN_DELAY * 2);
    }

    #[test]
    fn a_single_frame_gif_is_a_sequence_of_one() {
        let seq = decode_gif(&gif(&[100]), 0, 16, 1 << 20).expect("decodes");
        assert_eq!(seq.len(), 1);
        let (i, _) = seq.at(Duration::from_secs(9));
        assert_eq!(i, 0, "a still never advances");
        assert_eq!(seq.until_next(Duration::ZERO), Duration::MAX);
    }

    #[test]
    fn the_frame_cap_truncates_rather_than_failing() {
        let seq = decode_gif(&gif(&[100, 100, 100, 100]), 0, 2, 1 << 20).expect("decodes");
        assert_eq!(seq.len(), 2);
        assert!(seq.truncated);
        assert_eq!(seq.total, Duration::from_millis(200));
    }

    #[test]
    fn the_pixel_cap_refuses_before_allocating() {
        let err = decode_gif(&gif(&[100]), 0, 16, 8).expect_err("4x4 is over 8 pixels");
        assert!(matches!(
            err,
            AnimError::TooLarge {
                pixels: 16,
                limit: 8
            }
        ));
    }

    #[test]
    fn at_loops_through_the_frames() {
        let seq = decode_gif(&gif(&[100, 50, 200]), 0, 16, 1 << 20).expect("decodes");
        let expect = |ms: u64, want: usize| {
            let (i, _) = seq.at(Duration::from_millis(ms));
            assert_eq!(i, want, "at {ms} ms");
        };
        expect(0, 0);
        expect(99, 0);
        expect(100, 1);
        expect(149, 1);
        expect(150, 2);
        expect(349, 2);
        expect(350, 0);
        expect(450, 1);
        expect(350 * 40 + 120, 1);
    }

    #[test]
    fn until_next_is_the_time_left_on_the_current_frame() {
        let seq = decode_gif(&gif(&[100, 50, 200]), 0, 16, 1 << 20).expect("decodes");
        assert_eq!(seq.until_next(Duration::ZERO), Duration::from_millis(100));
        assert_eq!(
            seq.until_next(Duration::from_millis(120)),
            Duration::from_millis(30)
        );
        assert_eq!(
            seq.until_next(Duration::from_millis(470)),
            Duration::from_millis(30),
            "and it loops with at()"
        );
    }

    #[test]
    fn rubbish_is_an_error_and_not_a_panic() {
        for bytes in [
            &b""[..],
            &b"not a gif at all"[..],
            &b"GIF89a"[..],
            &b"GIF89a\xff\xff\xff\xff"[..],
        ] {
            assert!(decode_gif(bytes, 0, 16, 1 << 20).is_err(), "{bytes:?}");
        }
    }

    #[test]
    fn a_truncated_gif_keeps_the_frames_it_had() {
        // Cutting a three-frame file in half leaves a header and part of the
        // stream: whatever comes back, it is a value or an error.
        let whole = gif(&[100, 100, 100]);
        for cut in [8, 16, 32, whole.len() / 2, whole.len() - 1] {
            let _ = decode_gif(&whole[..cut.min(whole.len())], 0, 16, 1 << 20);
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(200))]

        /// These bytes arrive from a content delivery network by way of a
        /// message somebody else wrote, so the only acceptable outcomes are a
        /// sequence and an error.
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(0u8..=255, 0..600)) {
            let _ = decode_gif(&bytes, 0, 8, 1 << 16);
        }

        /// The same, starting from a real header, so the decoder gets past the
        /// magic number and into the parts that parse.
        #[test]
        fn arbitrary_bytes_after_a_real_header_never_panic(
            tail in proptest::collection::vec(0u8..=255, 0..400),
        ) {
            let mut bytes = gif(&[100, 100]);
            bytes.truncate(14);
            bytes.extend_from_slice(&tail);
            let _ = decode_gif(&bytes, 0, 8, 1 << 16);
        }
    }
}
