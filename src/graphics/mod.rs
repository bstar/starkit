//! Pictures, and getting them onto a terminal.
//!
//! Only the decoder guard is here so far; protocol detection, the cache and
//! the rasteriser follow with the rest of the terminal-graphics move. This
//! part arrived early because the Winamp skin importer decodes a bitmap out of
//! a downloaded archive, and that must not be the one decode with no limits
//! on it.
//!
//! Every image either application decodes came from outside it: embedded in a
//! tag, sitting in an album folder, inside a downloaded skin, or fetched from
//! a CDN. The `image` crate's own defaults allow any width and height and cap
//! only the total allocation, at 512 MiB -- so a file of a few kilobytes that
//! declares itself 10000x10000 is decoded into four hundred megabytes before
//! anything notices it is absurd, on a worker a track change is waiting for.
//!
//! Dimensions are checked against the header before the pixels are read, which
//! is the difference between refusing a lie and allocating for it.

/// The largest picture worth decoding, on a side.
///
/// Well above any real cover -- the Cover Art Archive's largest is 1200 -- and
/// far enough above a scan that nothing legitimate is refused.
pub const MAX_DIMENSION: u32 = 8192;

/// Decode `bytes`, refusing anything larger than `max_dim` on a side.
pub fn decode_limited(bytes: &[u8], max_dim: u32) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    reader.limits(limits(max_dim));
    reader.decode()
}

/// Decode the file at `path`, with the same limits.
///
/// Streams rather than reading the whole file first, so a huge file on disk is
/// refused after its header rather than after its bytes.
pub fn open_limited(
    path: &std::path::Path,
    max_dim: u32,
) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    reader.limits(limits(max_dim));
    reader.decode()
}

fn limits(max_dim: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    // Below the 512 MiB default, and still far more than any cover needs:
    // 8192 squared at four bytes a pixel is 256 MiB, so this permits the
    // largest picture the dimensions allow and nothing beyond it.
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header is a claim, not a fact. Refusing it costs a few bytes of
    /// parsing; believing it costs however much memory it asked for.
    #[test]
    fn a_picture_larger_than_the_limit_is_refused_before_it_is_decoded() {
        // 64x64 is a real image; the limit here is deliberately smaller.
        let small = image::RgbImage::from_pixel(64, 64, image::Rgb([1, 2, 3]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(small)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();

        assert!(
            decode_limited(&bytes, 32).is_err(),
            "a 64x64 picture was decoded under a 32-pixel limit"
        );
        let ok = decode_limited(&bytes, MAX_DIMENSION).expect("an ordinary picture was refused");
        assert_eq!((ok.width(), ok.height()), (64, 64));
    }

    #[test]
    fn rubbish_is_an_error_rather_than_a_panic() {
        for bytes in [&b""[..], &b"not a picture"[..], &[0xff; 64][..]] {
            assert!(decode_limited(bytes, MAX_DIMENSION).is_err());
        }
    }
}
