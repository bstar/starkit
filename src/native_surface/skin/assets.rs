//! Immutable PNG assets and themeable atlas sprites. No filesystem or transport policy.
use super::{inside, over, Insets, NineSlice};
use crate::{
    image::{self, ImageDecoder, RgbaImage},
    native_surface::PixelRect,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Cursor};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sprite {
    pub asset: String,
    /// Physical source pixels, including atlas offset.
    pub rect: PixelRect,
    /// Physical source pixels per logical design pixel.
    pub density: u16,
    /// Content insets in physical source pixels.
    pub content: Insets,
    pub nine_slice: Option<NineSlice>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub sprite: Sprite,
    /// Palette role; absent means retain the artwork's original RGBA colors.
    pub tint: Option<String>,
}
struct Asset {
    encoded: Vec<u8>,
    pixels: RgbaImage,
}
/// Budget includes both compressed and decoded retained storage. Assets cannot
/// change under an existing ID: callers create a new ID for a new revision.
/// No global cache, disk I/O or hidden lifetime outside this owner's storage.
pub struct AssetCache {
    assets: BTreeMap<String, Asset>,
    budget: usize,
    retained: usize,
}
impl AssetCache {
    pub fn new(budget: usize) -> Self {
        Self {
            assets: BTreeMap::new(),
            budget: budget.min(128_000_000),
            retained: 0,
        }
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
    pub fn image(&self, id: &str) -> Result<&RgbaImage> {
        Ok(&self.assets.get(id).context("Missing skin asset")?.pixels)
    }
    pub fn contains(&self, id: &str) -> bool {
        self.assets.contains_key(id)
    }
    pub fn insert_png(&mut self, id: &str, encoded: &[u8]) -> Result<()> {
        ensure!(
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b)),
            "Invalid skin asset ID"
        );
        if let Some(old) = self.assets.get(id) {
            ensure!(old.encoded == encoded, "Skin asset ID cannot be replaced");
            return Ok(());
        }
        ensure!(
            encoded.len() <= 8_000_000
                && encoded.len() <= self.budget.saturating_sub(self.retained),
            "Skin asset exceeds cache budget"
        );
        let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(encoded))
            .context("Decode skin PNG header")?;
        let (w, h) = decoder.dimensions();
        ensure!(
            w > 0 && h > 0 && w <= 8192 && h <= 8192,
            "Invalid skin asset dimensions"
        );
        let bytes = u64::from(w) * u64::from(h) * 4;
        let total = bytes + encoded.len() as u64;
        ensure!(
            total <= self.budget.saturating_sub(self.retained) as u64,
            "Decoded skin asset exceeds cache budget"
        );
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(self.budget.saturating_sub(self.retained + encoded.len()) as u64);
        decoder.set_limits(limits)?;
        let pixels = image::DynamicImage::from_decoder(decoder)?.to_rgba8();
        self.assets.insert(
            id.into(),
            Asset {
                encoded: encoded.to_vec(),
                pixels,
            },
        );
        self.retained += total as usize;
        Ok(())
    }
    pub fn remove(&mut self, id: &str) {
        if let Some(old) = self.assets.remove(id) {
            self.retained -= old.encoded.len() + old.pixels.as_raw().len();
        }
    }
    pub fn clear(&mut self) {
        self.assets.clear();
        self.retained = 0;
    }
    pub fn sprite(&self, sprite: &Sprite) -> Result<RgbaImage> {
        let source = &self
            .assets
            .get(&sprite.asset)
            .context("Missing skin asset")?
            .pixels;
        ensure!((1..=4).contains(&sprite.density), "Invalid sprite density");
        let r = sprite.rect;
        ensure!(
            r.width > 0 && r.height > 0 && inside(source, r),
            "Sprite outside atlas"
        );
        let i = sprite.content;
        ensure!(
            u32::from(i.left) + u32::from(i.right) <= u32::from(r.width)
                && u32::from(i.top) + u32::from(i.bottom) <= u32::from(r.height),
            "Invalid sprite content insets"
        );
        Ok(image::imageops::crop_imm(
            source,
            r.x.into(),
            r.y.into(),
            r.width.into(),
            r.height.into(),
        )
        .to_image())
    }
    /// Precompose layered masks at the selected density. Each layer occupies
    /// the same destination box. Unknown palette roles fail before painting.
    pub fn compose(
        &self,
        width: u16,
        height: u16,
        layers: &[Layer],
        palette: &BTreeMap<String, [u8; 3]>,
    ) -> Result<RgbaImage> {
        ensure!(
            width > 0
                && height > 0
                && u32::from(width) * u32::from(height) <= 8_000_000
                && layers.len() <= 64,
            "Invalid layered sprite dimensions"
        );
        if let Some(first) = layers.first() {
            ensure!(
                layers
                    .iter()
                    .all(|l| l.sprite.density == first.sprite.density),
                "Layer densities must match"
            );
        }
        for layer in layers {
            if let Some(role) = &layer.tint {
                ensure!(palette.contains_key(role), "Missing skin tint role {role}");
            }
        }
        let mut result = RgbaImage::new(width.into(), height.into());
        for layer in layers {
            let mut source = self.sprite(&layer.sprite)?;
            if let Some(role) = &layer.tint {
                let c = palette[role];
                for p in source.pixels_mut() {
                    p[0] = c[0];
                    p[1] = c[1];
                    p[2] = c[2];
                }
            }
            if let Some(nine) = layer.sprite.nine_slice {
                nine.paint(&source, &mut result, PixelRect::new(0, 0, width, height))?;
            } else {
                ensure!(
                    source.dimensions() == (width.into(), height.into()),
                    "Sprite scaling requires explicit nine-slice metadata"
                );
                for (dst, src) in result.pixels_mut().zip(source.pixels()) {
                    over(dst, *src);
                }
            }
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Rgba;
    fn png(c: [u8; 4]) -> Vec<u8> {
        let im = RgbaImage::from_pixel(2, 2, Rgba(c));
        let mut out = Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(im)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }
    fn sprite() -> Sprite {
        Sprite {
            asset: "mask".into(),
            rect: PixelRect::new(0, 0, 2, 2),
            density: 1,
            content: Insets {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            nine_slice: None,
        }
    }
    #[test]
    fn immutable_ids_budget_and_reclamation() {
        let bytes = png([1, 2, 3, 255]);
        let mut cache = AssetCache::new(bytes.len() + 16);
        cache.insert_png("mask", &bytes).unwrap();
        let retained = cache.retained_bytes();
        cache.insert_png("mask", &bytes).unwrap();
        assert_eq!(cache.retained_bytes(), retained);
        assert!(cache.insert_png("mask", &png([4, 5, 6, 255])).is_err());
        assert!(cache.insert_png("extra", &bytes).is_err());
        cache.remove("mask");
        assert_eq!(cache.retained_bytes(), 0);
        cache.insert_png("new", &bytes).unwrap();
        cache.clear();
        assert_eq!(cache.retained_bytes(), 0);
    }
    #[test]
    fn tint_layers_and_atlas_bounds_are_explicit() {
        let mut cache = AssetCache::new(10000);
        cache
            .insert_png("mask", &png([255, 255, 255, 128]))
            .unwrap();
        let layer = Layer {
            sprite: sprite(),
            tint: Some("accent".into()),
        };
        let palette = [("accent".into(), [10, 20, 30])].into();
        let im = cache
            .compose(2, 2, std::slice::from_ref(&layer), &palette)
            .unwrap();
        assert_eq!(im.get_pixel(0, 0), &Rgba([10, 20, 30, 128]));
        assert!(cache
            .compose(2, 2, std::slice::from_ref(&layer), &BTreeMap::new())
            .is_err());
        let mut bad = sprite();
        bad.rect.x = 1;
        assert!(cache.sprite(&bad).is_err());
        bad = sprite();
        bad.density = 0;
        assert!(cache.sprite(&bad).is_err());
        let mut other = layer.clone();
        other.sprite.density = 2;
        assert!(cache
            .compose(2, 2, &[layer.clone(), other], &palette)
            .is_err());
        assert!(cache.compose(4, 4, &[layer], &palette).is_err());
    }
}
