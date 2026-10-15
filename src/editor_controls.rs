//! Render-only editor controls. Timeline ownership and edit commands stay in applications.
use crate::native_surface::{HitRegion, PixelRect, Primitive, Surface};
use anyhow::{ensure, Result};

/// Frame-based HH:MM:SS:FF display for non-drop-frame project rates.
pub fn timecode(seconds: f64, fps: u32) -> String {
    let fps = u64::from(fps.max(1));
    let frames = (seconds.max(0.) * fps as f64).round() as u64;
    let seconds = frames / fps;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        frames % fps
    )
}
#[derive(Clone, Copy, Debug)]
pub struct TimeView {
    pub first_frame: i64,
    pub frames_per_pixel: f64,
    pub rect: PixelRect,
}
impl TimeView {
    pub fn new(first_frame: i64, frames_per_pixel: f64, rect: PixelRect) -> Result<Self> {
        ensure!(
            frames_per_pixel.is_finite() && frames_per_pixel > 0. && rect.width > 0,
            "Invalid timeline viewport"
        );
        Ok(Self {
            first_frame,
            frames_per_pixel,
            rect,
        })
    }
    pub fn frame_at(self, x: f64) -> i64 {
        (self.first_frame as f64 + (x - f64::from(self.rect.x)) * self.frames_per_pixel)
            .round()
            .clamp(0., i64::MAX as f64) as i64
    }
    pub fn x_at(self, frame: i64) -> f64 {
        f64::from(self.rect.x) + (frame as f64 - self.first_frame as f64) / self.frames_per_pixel
    }
    pub fn snap(self, frame: i64, anchors: impl IntoIterator<Item = i64>, pixels: f64) -> i64 {
        anchors
            .into_iter()
            .filter(|a| (*a as f64 - frame as f64).abs() <= pixels.max(0.) * self.frames_per_pixel)
            .min_by_key(|a| a.abs_diff(frame))
            .unwrap_or(frame)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Slider {
    pub rect: PixelRect,
    pub min: f64,
    pub max: f64,
}
impl Slider {
    pub fn new(rect: PixelRect, min: f64, max: f64) -> Result<Self> {
        ensure!(
            min.is_finite() && max.is_finite() && max > min && rect.width > 0,
            "Invalid slider"
        );
        Ok(Self { rect, min, max })
    }
    pub fn value_at(self, x: f64) -> f64 {
        self.min
            + ((x - f64::from(self.rect.x)) / f64::from(self.rect.width)).clamp(0., 1.)
                * (self.max - self.min)
    }
    pub fn paint(self, surface: &mut Surface, value: f64, action: &str, accent: &str) {
        let fraction = ((value - self.min) / (self.max - self.min)).clamp(0., 1.);
        surface.fill(self.rect, "#303849", self.rect.height / 2);
        let mut fill = self.rect;
        fill.width = (fraction * f64::from(fill.width)).round() as u16;
        if fill.width > 0 {
            surface.fill(fill, accent, fill.height / 2);
        }
        surface.hits.push(HitRegion {
            rect: self.rect,
            action: action.into(),
        });
    }
}
pub fn button(surface: &mut Surface, rect: PixelRect, label: &str, action: &str, active: bool) {
    crate::native_surface::style::Style::default().button(surface, rect, label, action, active);
}
/// Visible, downsampled peaks only. This never scans an audio file on the UI thread.
pub fn waveform(surface: &mut Surface, rect: PixelRect, peaks: &[[f32; 2]], color: &str) {
    if rect.width == 0 || rect.height < 2 || peaks.len() < 2 {
        return;
    }
    let count = peaks.len().min(512);
    let center = rect.height / 2;
    let points = (0..count)
        .map(|i| {
            let p = peaks[i * peaks.len() / count];
            let peak = p[0].abs().max(p[1].abs()).clamp(0., 1.);
            [
                (i * usize::from(rect.width.saturating_sub(1)) / (count - 1)) as u16,
                (f32::from(center) - peak * f32::from(center.saturating_sub(1))).round() as u16,
            ]
        })
        .collect();
    surface.nodes.push(Primitive::Path {
        rect,
        points,
        color: color.into(),
        width: 1,
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timecode_rounds_at_frame_boundaries() {
        assert_eq!(timecode(59.999, 60), "00:01:00:00");
        assert_eq!(timecode(3600.5, 30), "01:00:00:15");
        assert_eq!(timecode(-5., 60), "00:00:00:00");
    }
    #[test]
    fn timeline_zoom_changes_hit_geometry_and_snap_distance() {
        let a = TimeView::new(100, 2., PixelRect::new(20, 0, 500, 40)).unwrap();
        assert_eq!(a.frame_at(30.), 120);
        assert_eq!(a.x_at(120), 30.);
        assert_eq!(a.snap(120, [123, 130], 2.), 123);
        assert_eq!(a.snap(120, [125], 2.), 120);
    }
}
