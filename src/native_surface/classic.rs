//! Palette-derived bevels and displays for compact native desktop controls.
//! Application state, layout and action names remain with the application.
use super::{HitRegion, PixelRect as R, Primitive, Surface};

#[derive(Clone, Debug)]
pub struct Colors {
    pub panel: String,
    pub inset: String,
    pub ink: String,
    pub dim: String,
    pub accent: String,
    pub highlight: String,
    pub shadow: String,
}
fn blend(a: [u8; 3], b: [u8; 3], amount: u16) -> String {
    let mut c = [0; 3];
    for i in 0..3 {
        c[i] = ((u16::from(a[i]) * (256 - amount) + u16::from(b[i]) * amount) / 256) as u8;
    }
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}
impl Colors {
    pub fn new(bg: [u8; 3], fg: [u8; 3], dim: [u8; 3], accent: [u8; 3], border: [u8; 3]) -> Self {
        Self {
            panel: blend(bg, border, 48),
            inset: blend(bg, [0; 3], 36),
            ink: blend(fg, fg, 0),
            dim: blend(dim, dim, 0),
            accent: blend(accent, accent, 0),
            highlight: blend(border, fg, 60),
            shadow: blend(bg, [0; 3], 120),
        }
    }
}
/// Draw a bounded frame. A single radius controls its outer corners only;
/// buttons deliberately have square corners.
pub fn frame(s: &mut Surface, rect: R, colors: &Colors, radius: u16, inset: bool) {
    if rect.width < 2 || rect.height < 2 {
        return;
    }
    s.fill(
        rect,
        if inset { &colors.inset } else { &colors.panel },
        radius,
    );
    s.nodes.push(Primitive::Border {
        rect,
        color: if inset {
            colors.shadow.clone()
        } else {
            colors.highlight.clone()
        },
        radius,
    });
    if radius == 0 {
        let (top, bottom) = if inset {
            (&colors.shadow, &colors.highlight)
        } else {
            (&colors.highlight, &colors.shadow)
        };
        s.fill(R::new(rect.x, rect.y, rect.width, 1), top, 0);
        s.fill(R::new(rect.x, rect.y, 1, rect.height), top, 0);
        s.fill(
            R::new(rect.x, rect.y + rect.height - 1, rect.width, 1),
            bottom,
            0,
        );
        s.fill(
            R::new(rect.x + rect.width - 1, rect.y, 1, rect.height),
            bottom,
            0,
        );
    }
}
pub fn label(
    s: &mut Surface,
    rect: R,
    text: impl Into<String>,
    color: &str,
    size: u16,
    bold: bool,
) {
    s.nodes.push(Primitive::Text {
        rect,
        text: text.into(),
        color: color.into(),
        size: size.clamp(1, 128),
        bold,
        mono: true,
    });
}
#[allow(clippy::too_many_arguments)]
pub fn button(
    s: &mut Surface,
    rect: R,
    colors: &Colors,
    text: &str,
    action: &str,
    size: u16,
    active: bool,
) {
    frame(s, rect, colors, 0, active);
    let pad = 6.min(rect.width / 4);
    label(
        s,
        R::new(
            rect.x + pad,
            rect.y,
            rect.width.saturating_sub(pad * 2),
            rect.height,
        ),
        text,
        if active { &colors.accent } else { &colors.ink },
        size,
        false,
    );
    s.hits.push(HitRegion {
        rect,
        action: action.into(),
    });
}
/// Seven-segment clock drawn as geometry, independent of font fallback.
pub fn clock(s: &mut Surface, rect: R, text: &str, color: &str) {
    let count = text.chars().count().max(1) as u16;
    let step = rect.width / count;
    if step < 5 || rect.height < 9 {
        return;
    }
    let thick = (rect.height / 12).max(1);
    let width = step.saturating_sub(thick * 3).max(thick);
    let half = rect.height / 2;
    for (index, ch) in text.chars().enumerate() {
        let x = rect.x + index as u16 * step;
        if ch == ':' {
            for y in [rect.y + rect.height / 3, rect.y + rect.height * 2 / 3] {
                s.fill(R::new(x + width / 2, y, thick, thick), color, 0);
            }
            continue;
        }
        let bits = match ch {
            '0' => 0b0111111,
            '1' => 0b0000110,
            '2' => 0b1011011,
            '3' => 0b1001111,
            '4' => 0b1100110,
            '5' => 0b1101101,
            '6' => 0b1111101,
            '7' => 0b0000111,
            '8' => 0b1111111,
            '9' => 0b1101111,
            _ => 0,
        };
        let segments = [
            R::new(x + thick, rect.y, width.saturating_sub(thick), thick),
            R::new(x + width, rect.y + thick, thick, half.saturating_sub(thick)),
            R::new(x + width, rect.y + half, thick, half.saturating_sub(thick)),
            R::new(
                x + thick,
                rect.y + rect.height - thick,
                width.saturating_sub(thick),
                thick,
            ),
            R::new(x, rect.y + half, thick, half.saturating_sub(thick)),
            R::new(x, rect.y + thick, thick, half.saturating_sub(thick)),
            R::new(
                x + thick,
                rect.y + half - thick / 2,
                width.saturating_sub(thick),
                thick,
            ),
        ];
        for (bit, segment) in segments.into_iter().enumerate() {
            if bits & (1 << bit) != 0 {
                s.fill(segment, color, 0);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_clocks_and_frames_remain_bounded() {
        for w in [20, 80, 160] {
            let mut s = Surface::new(w, 60, "#101010".into());
            let c = Colors::new([16; 3], [240; 3], [128; 3], [160, 220, 140], [80; 3]);
            frame(&mut s, R::new(0, 0, w, 60), &c, 8, false);
            clock(&mut s, R::new(2, 4, w - 4, 40), "01:09", &c.accent);
            assert!(s.validate().is_ok());
        }
    }
}
