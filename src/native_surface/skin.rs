//! Bitmap skin composition, independent of applications and terminal transport.
//! Source artwork is immutable; resize layout boxes rather than finished frames.
use super::PixelRect;
use crate::image::{Rgba, RgbaImage};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Insets {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Stretch,
    Tile,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct NineSlice {
    pub insets: Insets,
    #[serde(default)]
    pub horizontal: Repeat,
    #[serde(default)]
    pub vertical: Repeat,
}
fn inside(image: &RgbaImage, rect: PixelRect) -> bool {
    u32::from(rect.x) + u32::from(rect.width) <= image.width()
        && u32::from(rect.y) + u32::from(rect.height) <= image.height()
}
fn over(dst: &mut Rgba<u8>, src: Rgba<u8>) {
    if src[3] == 0 {
        return;
    }
    if src[3] == 255 {
        *dst = src;
        return;
    }
    let a = u32::from(src[3]);
    let back = u32::from(dst[3]) * (255 - a);
    let out = a * 255 + back;
    if out == 0 {
        return;
    }
    for c in 0..3 {
        dst[c] = ((u32::from(src[c]) * a * 255 + u32::from(dst[c]) * back + out / 2) / out) as u8;
    }
    dst[3] = ((out + 127) / 255) as u8;
}
fn map(index: u32, source: u32, destination: u32, repeat: Repeat) -> u32 {
    match repeat {
        Repeat::Tile => index % source,
        Repeat::Stretch => index * source / destination,
    }
}
impl NineSlice {
    /// Composite with unscaled corners. For a density-specific atlas, render to
    /// density-specific destination pixels. Stretch and tile use nearest samples.
    pub fn paint(&self, source: &RgbaImage, target: &mut RgbaImage, rect: PixelRect) -> Result<()> {
        ensure!(inside(target, rect), "skin destination outside target");
        ensure!(
            u64::from(target.width()) * u64::from(target.height()) <= 32_000_000,
            "skin target exceeds budget"
        );
        let i = self.insets;
        let [left, top, right, bottom] = [i.left, i.top, i.right, i.bottom].map(u32::from);
        ensure!(
            source.width() > left + right && source.height() > top + bottom,
            "skin source has no center"
        );
        let [w, h] = [u32::from(rect.width), u32::from(rect.height)];
        ensure!(
            w >= left + right && h >= top + bottom,
            "skin destination smaller than corners"
        );
        let sx = [0, left, source.width() - right, source.width()];
        let sy = [0, top, source.height() - bottom, source.height()];
        let dx = [0, left, w - right, w];
        let dy = [0, top, h - bottom, h];
        for row in 0..3 {
            for col in 0..3 {
                let sw = sx[col + 1] - sx[col];
                let sh = sy[row + 1] - sy[row];
                let dw = dx[col + 1] - dx[col];
                let dh = dy[row + 1] - dy[row];
                for y in 0..dh {
                    for x in 0..dw {
                        let px = sx[col]
                            + map(
                                x,
                                sw,
                                dw,
                                if col == 1 {
                                    self.horizontal
                                } else {
                                    Repeat::Stretch
                                },
                            );
                        let py = sy[row]
                            + map(
                                y,
                                sh,
                                dh,
                                if row == 1 {
                                    self.vertical
                                } else {
                                    Repeat::Stretch
                                },
                            );
                        over(
                            target.get_pixel_mut(
                                u32::from(rect.x) + dx[col] + x,
                                u32::from(rect.y) + dy[row] + y,
                            ),
                            *source.get_pixel(px, py),
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
/// Recolor a coverage mask without baking a theme's RGB values into the atlas.
pub fn tint_mask(mask: &RgbaImage, color: [u8; 3]) -> RgbaImage {
    RgbaImage::from_fn(mask.width(), mask.height(), |x, y| {
        Rgba([color[0], color[1], color[2], mask.get_pixel(x, y)[3]])
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Glyph {
    pub rect: PixelRect,
    pub advance: u16,
}
/// Fixed labels only. Applications retain shaped outline fonts for arbitrary
/// user text and fallback; absent glyphs are errors rather than silent blanks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BitmapFont {
    pub height: u16,
    pub baseline: u16,
    pub glyphs: BTreeMap<char, Glyph>,
}
impl BitmapFont {
    pub fn measure(&self, text: &str) -> Result<u32> {
        text.chars().try_fold(0u32, |width, ch| {
            let glyph = self
                .glyphs
                .get(&ch)
                .ok_or_else(|| anyhow::anyhow!("bitmap font missing {ch:?}"))?;
            width
                .checked_add(u32::from(glyph.advance))
                .ok_or_else(|| anyhow::anyhow!("bitmap text too wide"))
        })
    }
    pub fn paint(
        &self,
        atlas: &RgbaImage,
        target: &mut RgbaImage,
        rect: PixelRect,
        text: &str,
        color: [u8; 3],
    ) -> Result<()> {
        ensure!(inside(target, rect), "bitmap text outside target");
        ensure!(
            self.baseline <= self.height && self.height <= rect.height,
            "invalid bitmap text metrics"
        );
        ensure!(
            self.measure(text)? <= u32::from(rect.width),
            "bitmap label does not fit"
        );
        // Validate before drawing so malformed fonts never partially paint labels.
        for ch in text.chars() {
            let glyph = &self.glyphs[&ch];
            ensure!(
                inside(atlas, glyph.rect)
                    && glyph.rect.height <= self.height
                    && glyph.rect.width <= glyph.advance,
                "invalid bitmap glyph"
            );
        }
        let mut x = u32::from(rect.x);
        for ch in text.chars() {
            let glyph = &self.glyphs[&ch];
            for gy in 0..u32::from(glyph.rect.height) {
                for gx in 0..u32::from(glyph.rect.width) {
                    let alpha = atlas
                        .get_pixel(u32::from(glyph.rect.x) + gx, u32::from(glyph.rect.y) + gy)[3];
                    over(
                        target.get_pixel_mut(x + gx, u32::from(rect.y) + gy),
                        Rgba([color[0], color[1], color[2], alpha]),
                    );
                }
            }
            x += u32::from(glyph.advance);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skin_alpha_preserves_transparency_and_blends_partial_coverage() {
        let mut dst = Rgba([10, 20, 30, 255]);
        over(&mut dst, Rgba([200, 100, 0, 0]));
        assert_eq!(dst, Rgba([10, 20, 30, 255]));
        over(&mut dst, Rgba([200, 100, 0, 255]));
        assert_eq!(dst, Rgba([200, 100, 0, 255]));
        let mut clear = Rgba([0, 0, 0, 0]);
        over(&mut clear, Rgba([200, 100, 0, 128]));
        assert_eq!(clear, Rgba([200, 100, 0, 128]));
        over(&mut dst, Rgba([0, 0, 0, 128]));
        assert_eq!(dst, Rgba([100, 50, 0, 255]));
    }

    #[test]
    fn corners_survive_resizing_and_centers_tile() {
        let source = RgbaImage::from_fn(5, 5, |x, y| Rgba([x as u8 * 40, y as u8 * 40, 0, 255]));
        let nine = NineSlice {
            insets: Insets {
                left: 1,
                top: 1,
                right: 1,
                bottom: 1,
            },
            horizontal: Repeat::Tile,
            vertical: Repeat::Tile,
        };
        let mut target = RgbaImage::new(12, 10);
        nine.paint(&source, &mut target, PixelRect::new(1, 1, 10, 8))
            .unwrap();
        assert_eq!(target.get_pixel(1, 1), source.get_pixel(0, 0));
        assert_eq!(target.get_pixel(10, 8), source.get_pixel(4, 4));
        assert_eq!(target.get_pixel(2, 2), target.get_pixel(5, 5));
        assert_eq!(target.get_pixel(0, 0)[3], 0);
        assert!(nine
            .paint(&source, &mut target, PixelRect::new(0, 0, 1, 1))
            .is_err());
    }
    #[test]
    fn bitmap_labels_are_bounded_and_missing_glyphs_fail() {
        let atlas = RgbaImage::from_pixel(2, 2, Rgba([255; 4]));
        let font = BitmapFont {
            height: 2,
            baseline: 2,
            glyphs: [(
                'A',
                Glyph {
                    rect: PixelRect::new(0, 0, 2, 2),
                    advance: 3,
                },
            )]
            .into(),
        };
        let mut target = RgbaImage::new(6, 2);
        font.paint(
            &atlas,
            &mut target,
            PixelRect::new(0, 0, 6, 2),
            "AA",
            [10, 20, 30],
        )
        .unwrap();
        assert_eq!(target.get_pixel(3, 1), &Rgba([10, 20, 30, 255]));
        assert!(font
            .paint(
                &atlas,
                &mut target,
                PixelRect::new(0, 0, 5, 2),
                "AA",
                [0; 3]
            )
            .is_err());
        assert!(font.measure("B").is_err());
    }
}
