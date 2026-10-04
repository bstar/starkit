//! Local pointer feedback: never wait for the remote controller on hover.
use crate::native_surface::PixelRect;
use std::io::Write;

#[derive(Default)]
pub(super) struct Pointer {
    resizing: bool,
    captured: bool,
}
impl Pointer {
    pub fn update(
        &mut self,
        action: &str,
        left: bool,
        pixel: [u32; 2],
        handles: &[PixelRect],
        out: &mut impl Write,
    ) -> std::io::Result<()> {
        let hit = handles.iter().any(|r| {
            pixel[0] >= u32::from(r.x)
                && pixel[0] < u32::from(r.x) + u32::from(r.width)
                && pixel[1] >= u32::from(r.y)
                && pixel[1] < u32::from(r.y) + u32::from(r.height)
        });
        if action == "down" && left {
            self.captured = hit;
        }
        if action == "up" {
            self.captured = false;
        }
        self.set(self.captured || hit, out)
    }
    fn set(&mut self, resizing: bool, out: &mut impl Write) -> std::io::Result<()> {
        if resizing != self.resizing {
            out.write_all(if resizing {
                b"\x1b]22;ns-resize\x1b\\"
            } else {
                b"\x1b]22;\x1b\\"
            })?;
            out.flush()?;
            self.resizing = resizing;
        }
        Ok(())
    }
    pub fn reset(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        self.captured = false;
        self.set(false, out)
    }
}
impl Drop for Pointer {
    fn drop(&mut self) {
        let _ = self.reset(&mut std::io::stdout().lock());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_capture_release_and_reset_use_fixed_sequences_only() {
        let mut p = Pointer::default();
        let mut out = vec![];
        let regions = [PixelRect::new(0, 10, 100, 16)];
        p.update("move", false, [50, 15], &regions, &mut out)
            .unwrap();
        assert_eq!(out, b"\x1b]22;ns-resize\x1b\\");
        p.update("down", true, [50, 15], &regions, &mut out)
            .unwrap();
        p.update("drag", true, [50, 90], &[], &mut out).unwrap();
        assert!(p.resizing);
        assert_eq!(out.len(), b"\x1b]22;ns-resize\x1b\\".len());
        p.update("up", true, [50, 90], &regions, &mut out).unwrap();
        assert!(!p.resizing);
        p.update("down", true, [50, 15], &regions, &mut out)
            .unwrap();
        p.reset(&mut out).unwrap();
        assert!(!p.captured && !p.resizing);
        assert!(out.ends_with(b"\x1b]22;\x1b\\"));
    }
}
