//! One cancellable-by-replacement thumbnail queue per application session.
use crate::image::{DynamicImage, ImageFormat, RgbaImage};
use base64::Engine;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::Cursor;
use std::sync::Arc;

pub struct Thumbnailer {
    request: Sender<(String, Arc<RgbaImage>)>,
    stale: Receiver<(String, Arc<RgbaImage>)>,
    pub output: Receiver<(String, String)>,
}
impl Default for Thumbnailer {
    fn default() -> Self {
        let (request, rx) = bounded::<(String, Arc<RgbaImage>)>(1);
        let stale = rx.clone();
        let (tx, output) = bounded(1);
        let old = output.clone();
        std::thread::spawn(move || {
            while let Ok((id, image)) = rx.recv() {
                let image = DynamicImage::ImageRgba8((*image).clone());
                // Never enlarge a source before the renderer chooses its sampling policy.
                let image = if image.width() > 1280 || image.height() > 1280 {
                    image.thumbnail(1280, 1280)
                } else {
                    image
                };
                let mut png = Cursor::new(vec![]);
                if image.write_to(&mut png, ImageFormat::Png).is_err() {
                    continue;
                }
                let result = (
                    id,
                    base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
                );
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
    pub fn request(&self, id: String, image: Arc<RgbaImage>) {
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
