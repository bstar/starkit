//! Pixel mouse mode belongs to the frontend, never to an application's hit tests.
use super::protocol::Viewport;
use std::io::{self, Write};

pub(super) fn supported(graphics: bool, measured: bool, term: Option<&str>, mux: bool) -> bool {
    graphics && measured && term == Some("xterm-kitty") && !mux
}

pub(super) struct Mode(bool);
impl Mode {
    pub(super) fn enable(enabled: bool) -> io::Result<Self> {
        let mode = Self(enabled);
        if enabled {
            let mut out = io::stdout().lock();
            out.write_all(b"\x1b[?1016h")?;
            out.flush()?;
        }
        Ok(mode)
    }
}
impl Drop for Mode {
    fn drop(&mut self) {
        if self.0 {
            let mut out = io::stdout().lock();
            let _ = out.write_all(b"\x1b[?1016l");
            let _ = out.flush();
        }
    }
}

/// Cell clients still receive logical cell coordinates; native hit tests retain
/// the original pixel position, without cell-centre rounding at any font scale.
pub(super) fn logical(pixel: [u32; 2], viewport: Viewport) -> [u16; 2] {
    let axis = |value: u32, pixels: u32, cells: u16| {
        (u64::from(value) * u64::from(cells) / u64::from(pixels.max(1)))
            .min(u64::from(cells.saturating_sub(1))) as u16
    };
    [
        axis(pixel[0], viewport.width, viewport.columns),
        axis(pixel[1], viewport.height, viewport.rows),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_support_alone_does_not_enable_pixel_mouse() {
        assert!(supported(true, true, Some("xterm-kitty"), false));
        assert!(!supported(true, true, Some("xterm-256color"), false));
        assert!(!supported(true, false, Some("xterm-kitty"), false));
        assert!(!supported(false, true, Some("xterm-kitty"), false));
        assert!(!supported(true, true, Some("xterm-kitty"), true));
    }
    #[test]
    fn pixel_motion_within_one_cell_survives_logical_projection() {
        for columns in [100, 80, 50] {
            let viewport = Viewport {
                width: 1000,
                height: 800,
                columns,
                rows: 40,
                generation: 1,
            };
            let points: Vec<_> = (200..210).map(|x| [x, 401]).collect();
            let cells: Vec<_> = points.iter().map(|p| logical(*p, viewport)).collect();
            assert!(cells.iter().all(|c| *c == cells[0]));
            assert_eq!(points.last().unwrap()[0] - points[0][0], 9);
            assert_eq!(logical([1000, 800], viewport), [columns - 1, 39]);
        }
    }
}
