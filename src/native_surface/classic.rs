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
/// Present an existing grid-based form through native primitives while retaining
/// its exact cell geometry for keyboard and pointer routing. This keeps mature
/// application forms usable while their dedicated compositions evolve.
pub fn form(
    buffer: &crate::ratatui::buffer::Buffer,
    width: u16,
    height: u16,
    font: u16,
    colors: &Colors,
    radius: u16,
) -> Surface {
    use crate::ratatui::style::{Color, Modifier};
    let area = buffer.area;
    let cw = f64::from(width) / f64::from(area.width.max(1));
    let ch = f64::from(height) / f64::from(area.height.max(1));
    let rect = |x: u16, y: u16, w: u16, h: u16| {
        R::new(
            (f64::from(x) * cw) as u16,
            (f64::from(y) * ch) as u16,
            ((f64::from(x + w) * cw) as u16).saturating_sub((f64::from(x) * cw) as u16),
            ((f64::from(y + h) * ch) as u16).saturating_sub((f64::from(y) * ch) as u16),
        )
    };
    let color = |c: Color, fallback: &str| match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.into(),
    };
    let mut surface = Surface::new(width, height, colors.panel.clone());
    for y in 0..area.height {
        for x in 0..area.width {
            let symbol = buffer[(area.x + x, area.y + y)].symbol();
            if !matches!(symbol, "┌" | "╭" | "╔" | "┏") {
                continue;
            }
            let right = (x + 1..area.width).find(|&xx| {
                matches!(
                    buffer[(area.x + xx, area.y + y)].symbol(),
                    "┐" | "╮" | "╗" | "┓"
                )
            });
            let bottom = (y + 1..area.height).find(|&yy| {
                matches!(
                    buffer[(area.x + x, area.y + yy)].symbol(),
                    "└" | "╰" | "╚" | "┗"
                )
            });
            if let (Some(right), Some(bottom)) = (right, bottom) {
                frame(
                    &mut surface,
                    rect(x, y, right - x + 1, bottom - y + 1),
                    colors,
                    radius,
                    false,
                );
            }
        }
    }
    for y in 0..area.height {
        let mut x = 0;
        while x < area.width {
            let start = x;
            let first = &buffer[(area.x + x, area.y + y)];
            let fg = first.fg;
            let bg = first.bg;
            let mods = first.modifier;
            let mut text = String::new();
            while x < area.width {
                let cell = &buffer[(area.x + x, area.y + y)];
                if cell.fg != fg || cell.bg != bg || cell.modifier != mods {
                    break;
                }
                if cell.diff_option != ratatui::buffer::CellDiffOption::Skip {
                    let symbol = cell.symbol();
                    if symbol
                        .chars()
                        .all(|c| "│║─━═┌┐└┘┏┓┗┛╔╗╚╝┬┴├┤┼╭╮╰╯".contains(c))
                    {
                        text.push(' ');
                    } else {
                        text.push_str(symbol);
                    }
                }
                x += 1;
            }
            if text.trim().is_empty() {
                continue;
            }
            let r = rect(start, y, x - start, 1);
            if matches!(bg, Color::Rgb(..)) {
                surface.fill(r, &color(bg, &colors.panel), 0);
            }
            label(
                &mut surface,
                r,
                text,
                &color(fg, &colors.ink),
                font,
                mods.contains(Modifier::BOLD),
            );
        }
    }
    surface
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forms_keep_selection_colors_and_bounds_without_text_borders() {
        use crate::ratatui::{
            buffer::Buffer,
            layout::Rect,
            style::{Color, Style},
            widgets::{Block, Borders, Widget},
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 8));
        Block::default()
            .borders(Borders::ALL)
            .title("Library")
            .render(buffer.area, &mut buffer);
        buffer.set_string(
            2,
            2,
            "Selected album",
            Style::default()
                .fg(Color::Rgb(240, 240, 240))
                .bg(Color::Rgb(30, 40, 50)),
        );
        let c = Colors::new([16; 3], [240; 3], [128; 3], [160, 220, 140], [80; 3]);
        let surface = form(&buffer, 400, 160, 16, &c, 8);
        surface.validate().unwrap();
        assert!(surface
            .nodes
            .iter()
            .any(|n| matches!(n,Primitive::Fill{color,..} if color=="#1e2832")));
        assert!(surface
            .nodes
            .iter()
            .any(|n| matches!(n,Primitive::Text{text,..} if text.contains("Selected album"))));
        assert!(surface.nodes.iter().all(
            |n| !matches!(n,Primitive::Text{text,..} if text.contains('┌')||text.contains('│'))
        ));
    }
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
