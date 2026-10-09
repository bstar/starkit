//! One cancellable-by-replacement thumbnail queue per application session.
use crate::image::{DynamicImage, GenericImageView as _, ImageFormat, RgbaImage};
use base64::Engine;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::Cursor;
use std::sync::Arc;

enum Source {
    Still(Arc<RgbaImage>),
    Raster(Arc<RgbaImage>),
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
                    Source::Raster(image) => encode_png(&image),
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
    /// Transport an already sized document raster losslessly, without a thumbnail pass.
    /// Encoding and validation retain the shared pixel and transport limits.
    pub fn request_raster(&self, id: String, image: Arc<RgbaImage>) {
        self.submit(id, Source::Raster(image));
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

/// Stable content ID for immutable skin artwork. Unlike previews, these small
/// assets survive scene replacement and preview-cache churn for a connection.
pub fn skin_id(png_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("skin/{:x}", Sha256::digest(png_bytes))
}
/// Encode original player artwork and its immutable transport ID together.
pub fn encode_skin(image: &RgbaImage) -> anyhow::Result<(String, String)> {
    let png = encode_png(image)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&png)?;
    Ok((skin_id(&bytes), png))
}
pub(super) const SKIN_COUNT: usize = 512;
const SKIN_ENCODED: usize = 8_000_000;
const SKIN_RETAINED: usize = 32_000_000;

pub(super) fn skin_payload(id: &str, png: &str) -> anyhow::Result<(Vec<u8>, usize)> {
    anyhow::ensure!(png.len() <= 1_000_000, "Skin asset exceeds transport limit");
    validate_png(png)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(png)?;
    anyhow::ensure!(id == skin_id(&bytes), "Skin content ID mismatch");
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    let retained = png.len() + width as usize * height as usize * 4;
    Ok((bytes, retained))
}

/// Reliable assets are cached independently of the replaceable scene queue.
/// The decoded-size budget anticipates the native painter's retained storage.
#[derive(Default)]
pub(super) struct FrontendAssets {
    previews: std::collections::HashMap<String, String>,
    skins: std::collections::HashMap<String, String>,
    skin_encoded: usize,
    skin_retained: usize,
}
impl FrontendAssets {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn insert(&mut self, id: String, png: String) -> anyhow::Result<()> {
        if id.starts_with("skin/") {
            if let Some(old) = self.skins.get(&id) {
                anyhow::ensure!(old == &png, "Immutable skin asset changed");
                return Ok(());
            }
            let (_, retained) = skin_payload(&id, &png)?;
            anyhow::ensure!(
                self.skins.len() < SKIN_COUNT,
                "Skin asset count exceeds budget"
            );
            anyhow::ensure!(
                self.skin_encoded + png.len() <= SKIN_ENCODED,
                "Skin asset transport exceeds budget"
            );
            anyhow::ensure!(
                self.skin_retained + retained <= SKIN_RETAINED,
                "Decoded skin assets exceed budget"
            );
            self.skin_encoded += png.len();
            self.skin_retained += retained;
            self.skins.insert(id, png);
        } else {
            validate_png(&png)?;
            if self.previews.len() > 8 {
                self.previews.clear();
            }
            self.previews.insert(id, png);
        }
        Ok(())
    }
    /// Also called after a late asset arrives. Only an actual payload change
    /// requests a redraw; unrelated artwork must not cause a repaint.
    pub fn hydrate(&self, scene: &mut super::protocol::Scene) -> bool {
        use super::protocol::Component;
        let mut changed = false;
        for component in &mut scene.components {
            match component {
                Component::Surface { surface, .. } => {
                    for (id, png) in &mut surface.assets {
                        if let Some(cached) = self.skins.get(id) {
                            if png.as_ref() != Some(cached) {
                                *png = Some(cached.clone());
                                changed = true;
                            }
                        }
                    }
                }
                Component::Image { id, png, .. } => {
                    if let Some(cached) = self.previews.get(id) {
                        if png.as_ref() != Some(cached) {
                            *png = Some(cached.clone());
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn skin_fixture() -> (String, String) {
        let png = encode_png(&RgbaImage::from_pixel(
            3,
            3,
            crate::image::Rgba([12, 34, 56, 255]),
        ))
        .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&png)
            .unwrap();
        (skin_id(&bytes), png)
    }
    fn skin_scene(id: &str) -> super::super::protocol::Scene {
        use super::super::protocol::{Component, Rect, Scene, Viewport};
        use crate::native_surface::{PixelRect, Primitive, Surface};
        let mut surface = Surface::new(20, 20, "#000000".into());
        surface.assets.insert(id.into(), None);
        surface.nodes.push(Primitive::Sprite {
            rect: PixelRect::new(0, 0, 20, 20),
            asset: id.into(),
            source: PixelRect::new(0, 0, 3, 3),
            insets: Some([1, 1, 1, 1]),
            tint: None,
        });
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(0, 0, 2, 2)),
            Viewport {
                columns: 2,
                rows: 2,
                ..Viewport::default()
            },
            1,
        );
        scene.components.push(Component::Surface {
            rect: Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            surface,
        });
        scene
    }
    #[test]
    fn skin_hydration_survives_dropped_scenes_late_assets_and_preview_churn() {
        let (id, png) = skin_fixture();
        let mut cache = FrontendAssets::default();
        let mut late = skin_scene(&id);
        assert!(!cache.hydrate(&mut late));
        cache.insert(id.clone(), png.clone()).unwrap();
        assert!(cache.hydrate(&mut late));
        assert!(!cache.hydrate(&mut late));
        let retained = cache.skin_retained;
        cache.insert(id.clone(), png.clone()).unwrap();
        assert_eq!(cache.skin_retained, retained);
        for n in 0..24 {
            cache.insert(format!("preview/{n}"), png.clone()).unwrap();
        }
        let mut newest = skin_scene(&id);
        assert!(cache.hydrate(&mut newest));
        assert!(cache.skins.contains_key(&id));
        cache.clear();
        assert!(!cache.hydrate(&mut skin_scene(&id)));
    }
    #[test]
    fn skin_assets_reject_wrong_content_and_bound_decoded_memory_before_paint() {
        let (id, png) = skin_fixture();
        let mut cache = FrontendAssets::default();
        assert!(cache
            .insert(format!("skin/{}", "0".repeat(64)), png.clone())
            .is_err());
        assert!(cache
            .insert(format!("skin/{}", id[5..].to_uppercase()), png.clone())
            .is_err());
        cache.insert(id.clone(), png.clone()).unwrap();
        let other = encode_png(&RgbaImage::from_pixel(
            3,
            3,
            crate::image::Rgba([1, 2, 3, 255]),
        ))
        .unwrap();
        assert!(cache.insert(id, other).is_err());
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(png)
            .unwrap();
        bytes[16..20].copy_from_slice(&3000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&2600u32.to_be_bytes());
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        cache.insert(skin_id(&bytes), encoded).unwrap();
        bytes[16..20].copy_from_slice(&2999u32.to_be_bytes());
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert!(cache.insert(skin_id(&bytes), encoded).is_err());
    }
    #[test]
    fn document_raster_preserves_fine_detail_above_thumbnail_size() {
        let source = RgbaImage::from_fn(1800, 40, |x, y| {
            let ink = if (x + y) % 2 == 0 { 0 } else { 255 };
            crate::image::Rgba([ink, ink, ink, 255])
        });
        let worker = Thumbnailer::default();
        worker.request_raster("document".into(), Arc::new(source.clone()));
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
