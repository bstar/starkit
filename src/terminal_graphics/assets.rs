//! One cancellable-by-replacement thumbnail queue per application session.
use crate::image::{DynamicImage, GenericImageView as _, ImageFormat, RgbaImage};
use base64::Engine;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::Cursor;
use std::sync::Arc;

enum Source {
    Still(Arc<RgbaImage>),
    Animation(Arc<crate::animation::Animation>),
}

pub struct Thumbnailer {
    request: Sender<(String, Source)>,
    stale: Receiver<(String, Source)>,
    pub output: Receiver<(String, String)>,
}
impl Default for Thumbnailer {
    fn default() -> Self {
        let (request, rx) = bounded::<(String, Source)>(1);
        let stale = rx.clone();
        let (tx, output) = bounded(1);
        let old = output.clone();
        std::thread::spawn(move || {
            while let Ok((id, image)) = rx.recv() {
                let encoded = match image {
                    Source::Still(image) => {
                        let image = DynamicImage::ImageRgba8((*image).clone());
                        let image = if image.width() > 1280 || image.height() > 1280 {
                            image.thumbnail(1280, 1280)
                        } else {
                            image
                        };
                        encode_png(&image.to_rgba8())
                    }
                    Source::Animation(animation) => encode_animation(&animation)
                        .or_else(|_| animation.frame(0).and_then(|image| encode_png(&image))),
                };
                let Ok(png) = encoded else { continue };
                let result = (id, png);
                if let Err(crossbeam_channel::TrySendError::Full(result)) = tx.try_send(result) {
                    let _ = old.try_recv();
                    let _ = tx.try_send(result);
                }
            }
        });
        Self {
            request,
            stale,
            output,
        }
    }
}
impl Thumbnailer {
    pub fn request_animation(&self, id: String, animation: Arc<crate::animation::Animation>) {
        self.submit(id, Source::Animation(animation));
    }
    pub fn request(&self, id: String, image: Arc<RgbaImage>) {
        self.submit(id, Source::Still(image));
    }
    fn submit(&self, id: String, image: Source) {
        if let Err(crossbeam_channel::TrySendError::Full(request)) =
            self.request.try_send((id, image))
        {
            let _ = self.stale.try_recv();
            let _ = self.request.try_send(request);
        }
    }
}

/// Encode a small already-decoded surface (for example player controls).
pub fn encode_png(image: &RgbaImage) -> anyhow::Result<String> {
    anyhow::ensure!(
        u64::from(image.width()) * u64::from(image.height()) <= 8_000_000,
        "Surface dimensions exceed limit"
    );
    let mut png = Cursor::new(vec![]);
    DynamicImage::ImageRgba8(image.clone()).write_to(&mut png, ImageFormat::Png)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    validate_png(&encoded)?;
    Ok(encoded)
}

/// Check dimensions before decoding untrusted remote preview data.
pub fn validate_png(png: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        png.len() <= 12_000_000,
        "Preview asset exceeds transport limit"
    );
    let bytes = base64::engine::general_purpose::STANDARD.decode(png)?;
    anyhow::ensure!(
        bytes.len() >= 24 && bytes[..8] == *b"\x89PNG\r\n\x1a\n" && bytes[12..16] == *b"IHDR",
        "Invalid preview PNG header"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    anyhow::ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 8_000_000,
        "Preview asset dimensions exceed limit"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiny_preview_retains_source_dimensions_and_colors() {
        let source = RgbaImage::from_fn(2, 1, |x, _| {
            crate::image::Rgba([x as u8 * 255, 20, 30, 255])
        });
        let worker = Thumbnailer::default();
        worker.request("pixel-art".into(), Arc::new(source.clone()));
        let (_, png) = worker
            .output
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(png)
            .unwrap();
        let decoded = crate::image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded, source);
    }
}

/// Transfer composited frames once; playback runs in the local native renderer.
pub fn encode_animation(animation: &crate::animation::Animation) -> anyhow::Result<String> {
    let (width, height) = animation.dimensions();
    let mut target = width.max(height).min(1280);
    loop {
        let (output_width, output_height) = if width.max(height) > target {
            let first = animation.frame(0)?;
            DynamicImage::ImageRgba8((*first).clone())
                .thumbnail(target, target)
                .dimensions()
        } else {
            (width, height)
        };
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, output_width, output_height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(animation.len() as u32, animation.plays)?;
            encoder.set_blend_op(png::BlendOp::Source)?;
            encoder.set_dispose_op(png::DisposeOp::None)?;
            let mut writer = encoder.write_header()?;
            for index in 0..animation.len() {
                let frame = animation.frame(index)?;
                let image = if frame.width().max(frame.height()) > target {
                    DynamicImage::ImageRgba8((*frame).clone())
                        .thumbnail(target, target)
                        .into_rgba8()
                } else {
                    (*frame).clone()
                };
                let mut numerator = animation.delay(index).as_millis().min(65_535_000) as u32;
                let mut denominator = 1000u32;
                let (mut a, mut b) = (numerator, denominator);
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                numerator /= a.max(1);
                denominator /= a.max(1);
                if numerator > 65535 {
                    numerator /= denominator;
                    denominator = 1;
                }
                writer.set_frame_delay(numerator as u16, denominator as u16)?;
                writer.write_image_data(image.as_raw())?;
            }
            writer.finish()?;
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        if validate_png(&encoded).is_ok() {
            return Ok(encoded);
        }
        anyhow::ensure!(target > 1, "Animation exceeds transport limit");
        target = (target / 2).max(1);
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;
    use crate::image::AnimationDecoder as _;
    #[test]
    fn animation_transport_preserves_alpha_timing_and_finite_loops() {
        let frames = vec![
            crate::anim::Frame {
                img: Arc::new(RgbaImage::from_pixel(
                    2,
                    2,
                    crate::image::Rgba([250, 20, 30, 255]),
                )),
                delay: std::time::Duration::from_millis(100),
            },
            crate::anim::Frame {
                img: Arc::new(RgbaImage::from_pixel(
                    2,
                    2,
                    crate::image::Rgba([0, 0, 0, 0]),
                )),
                delay: std::time::Duration::from_millis(200),
            },
        ];
        let seq = crate::anim::FrameSequence {
            frames,
            total: std::time::Duration::from_millis(300),
            plays: 2,
            id: 0,
            truncated: false,
        };
        let frames = seq
            .frames
            .iter()
            .map(|f| {
                Ok(crate::image::Frame::from_parts(
                    (*f.img).clone(),
                    0,
                    0,
                    crate::image::Delay::from_numer_denom_ms(f.delay.as_millis() as u32, 1),
                ))
            })
            .collect::<Vec<_>>();
        let animation = crate::animation::Animation::collect(
            crate::image::Frames::new(Box::new(frames.into_iter())),
            seq.plays,
            64_000_000,
            || false,
        )
        .unwrap();
        let encoded = encode_animation(&animation).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let decoder = crate::image::codecs::png::PngDecoder::new(Cursor::new(bytes))
            .unwrap()
            .apng()
            .unwrap();
        assert!(
            matches!(decoder.loop_count(), crate::image::metadata::LoopCount::Finite(n) if n.get() == 2)
        );
        let decoded = decoder.into_frames().collect_frames().unwrap();
        for (actual, expected) in decoded.iter().zip(&seq.frames) {
            assert_eq!(actual.buffer(), expected.img.as_ref());
            let (n, d) = actual.delay().numer_denom_ms();
            assert_eq!(u128::from(n / d), expected.delay.as_millis());
        }
    }
}
