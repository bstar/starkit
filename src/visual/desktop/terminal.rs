//! Native, styled terminal cells for embedded tools. No terminal transport or IO.
use super::{rgb24, Tokens};
use crate::{
    ratatui::{
        buffer::Buffer,
        style::{Color, Modifier},
    },
    theme::color::Rgb,
};
use gpui::{div, px, Div, FontWeight, ParentElement, Styled};

#[derive(Debug, PartialEq)]
struct Run {
    x: u16,
    y: u16,
    width: u16,
    text: String,
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

fn runs(buffer: &Buffer) -> Vec<Run> {
    let mut output: Vec<Run> = Vec::new();
    for y in buffer.area.y..buffer.area.bottom() {
        let mut x = buffer.area.x;
        while x < buffer.area.right() {
            let cell = &buffer[(x, y)];
            let width = crate::wrap::width_of(cell.symbol())
                .max(1)
                .min(buffer.area.right() - x);
            if let Some(last) = output.last_mut().filter(|run| {
                run.y == y - buffer.area.y
                    && run.x + run.width == x - buffer.area.x
                    && run.fg == cell.fg
                    && run.bg == cell.bg
                    && run.modifier == cell.modifier
            }) {
                last.text.push_str(cell.symbol());
                last.width += width;
            } else {
                output.push(Run {
                    x: x - buffer.area.x,
                    y: y - buffer.area.y,
                    width,
                    text: cell.symbol().to_owned(),
                    fg: cell.fg,
                    bg: cell.bg,
                    modifier: cell.modifier,
                });
            }
            x += width;
        }
    }
    output
}

fn color(value: Color, fallback: Rgb) -> Rgb {
    let index = match value {
        Color::Reset => return fallback,
        Color::Rgb(r, g, b) => return Rgb::new(r, g, b),
        Color::Indexed(n) => n,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    const ANSI: [u32; 16] = [
        0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0, 0x808080,
        0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    if index < 16 {
        let c = ANSI[usize::from(index)];
        Rgb::new((c >> 16) as u8, (c >> 8) as u8, c as u8)
    } else if index < 232 {
        let n = index - 16;
        let channel = |n| if n == 0 { 0 } else { 55 + n * 40 };
        Rgb::new(channel(n / 36), channel((n / 6) % 6), channel(n % 6))
    } else {
        let n = 8 + (index - 232) * 10;
        Rgb::new(n, n, n)
    }
}

/// Draw contiguous styles as native text runs, retaining logical cell positions.
/// Measure the same font used by the surface; terminal columns follow actual glyph advances.
pub fn metrics(window: &gpui::Window) -> (f32, f32) {
    let text = gpui::SharedString::from("M");
    let run = gpui::TextRun {
        len: 1,
        font: gpui::font("monospace"),
        color: gpui::rgb(0xffffff).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(text, px(14.), &[run], None);
    ((line.width / px(1.)).max(1.), 20.)
}
pub fn surface(buffer: &Buffer, cell_width: f32, row_height: f32, tokens: Tokens) -> Div {
    div()
        .relative()
        .overflow_hidden()
        .font_family("monospace")
        .text_size(px(14.))
        .line_height(px(row_height))
        .w(px(f32::from(buffer.area.width) * cell_width))
        .h(px(f32::from(buffer.area.height) * row_height))
        .children(runs(buffer).into_iter().map(|run| {
            let mut fg = color(run.fg, tokens.foreground);
            let mut bg = color(run.bg, tokens.background);
            if run.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut text = div()
                .absolute()
                .left(px(f32::from(run.x) * cell_width))
                .top(px(f32::from(run.y) * row_height))
                .w(px(f32::from(run.width) * cell_width))
                .h(px(row_height))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb24(fg))
                .bg(rgb24(bg));
            if run.modifier.contains(Modifier::BOLD) {
                text = text.font_weight(FontWeight::BOLD);
            }
            if run.modifier.contains(Modifier::ITALIC) {
                text = text.italic();
            }
            if run.modifier.contains(Modifier::UNDERLINED) {
                text = text.underline();
            }
            text.child(run.text)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratatui::{layout::Rect, style::Style};
    #[test]
    fn native_runs_preserve_wide_text_positions_and_styles() {
        let mut buffer = Buffer::empty(Rect::new(3, 4, 8, 1));
        buffer.set_string(3, 4, "界a", Style::default().fg(Color::Red));
        let runs = runs(&buffer);
        assert_eq!((runs[0].x, runs[0].y, runs[0].width), (0, 0, 3));
        assert_eq!(runs[0].text, "界a");
        assert_eq!(runs[1].x, 3);
    }
}
