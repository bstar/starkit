//! One fit/crop transform for live pixels, overlays and input. Units are pixels.
use crate::native_surface::PixelRect;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentView {
    pub source: [u32; 2],
    pub target: PixelRect,
}
impl ContentView {
    pub fn new(source: [u32; 2], target: PixelRect) -> Result<Self> {
        ensure!(
            source.iter().all(|n| (1..=8192).contains(n))
                && u64::from(source[0]) * u64::from(source[1]) <= 32_000_000,
            "Invalid content dimensions"
        );
        ensure!(
            target.width > 0 && target.height > 0,
            "Empty content viewport"
        );
        Ok(Self { source, target })
    }
    /// x, y, width, height of the fitted image, retaining fractional pixel edges.
    pub fn fitted(self) -> [f64; 4] {
        let scale = (f64::from(self.target.width) / f64::from(self.source[0].max(1)))
            .min(f64::from(self.target.height) / f64::from(self.source[1].max(1)));
        let width = f64::from(self.source[0]) * scale;
        let height = f64::from(self.source[1]) * scale;
        [
            f64::from(self.target.x) + (f64::from(self.target.width) - width) / 2.,
            f64::from(self.target.y) + (f64::from(self.target.height) - height) / 2.,
            width,
            height,
        ]
    }
    pub fn to_content(self, point: [f64; 2], captured: bool) -> Option<[f64; 2]> {
        if point.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let [x, y, w, h] = self.fitted();
        if w <= 0.
            || h <= 0.
            || (!captured
                && (point[0] < x || point[0] >= x + w || point[1] < y || point[1] >= y + h))
        {
            return None;
        }
        Some([
            ((point[0] - x) / w * f64::from(self.source[0]))
                .clamp(0., f64::from(self.source[0].saturating_sub(1))),
            ((point[1] - y) / h * f64::from(self.source[1]))
                .clamp(0., f64::from(self.source[1].saturating_sub(1))),
        ])
    }
    pub fn to_display(self, point: [f64; 2]) -> [f64; 2] {
        let [x, y, w, h] = self.fitted();
        [
            x + point[0] / f64::from(self.source[0].max(1)) * w,
            y + point[1] / f64::from(self.source[1].max(1)) * h,
        ]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn letterboxes_and_round_trips_without_cell_rounding() {
        let view = ContentView::new([1920, 1080], PixelRect::new(17, 29, 1000, 1000)).unwrap();
        assert_eq!(view.to_content([20., 40.], false), None);
        let p = [817.25, 634.5];
        let round = view.to_content(view.to_display(p), false).unwrap();
        assert!((round[0] - p[0]).abs() < 1e-9 && (round[1] - p[1]).abs() < 1e-9);
        assert_eq!(view.to_content([-500., -500.], true), Some([0., 0.]));
    }
    #[test]
    fn rejects_empty_and_oversized_geometry() {
        assert!(ContentView::new([0, 1080], PixelRect::new(0, 0, 1, 1)).is_err());
        assert!(ContentView::new([8192, 8192], PixelRect::new(0, 0, 1, 1)).is_err());
    }
}
