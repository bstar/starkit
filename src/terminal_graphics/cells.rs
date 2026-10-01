//! Cell presentation over the same persistent session protocol as pixel mode.
use std::io::{self, Write};

use anyhow::Result;
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver, Sender};

use crate::ratatui::{
    backend::{Backend, CrosstermBackend},
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Widget},
};

use super::{
    protocol::{Component, Scene},
    renderer::RenderMessage,
};

fn color(text: &str) -> Color {
    let Some(value) = text
        .strip_prefix('#')
        .filter(|v| v.len() == 6 && v.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return Color::Reset;
    };
    Color::Rgb(
        u8::from_str_radix(&value[..2], 16).unwrap(),
        u8::from_str_radix(&value[2..4], 16).unwrap(),
        u8::from_str_radix(&value[4..], 16).unwrap(),
    )
}

fn text(buffer: &mut Buffer, x: u16, y: u16, value: &str, width: u16, style: Style) {
    if !buffer.area.contains((x, y).into()) {
        return;
    }
    let safe: String = value.chars().filter(|c| !c.is_control()).collect();
    buffer.set_stringn(
        x,
        y,
        safe,
        usize::from(width.min(buffer.area.right() - x)),
        style,
    );
}

pub(crate) fn buffer(scene: &Scene) -> Buffer {
    let area = Rect::new(0, 0, scene.viewport.columns, scene.viewport.rows);
    let mut buffer = Buffer::empty(area);
    let base = Style::default()
        .fg(color(&scene.foreground))
        .bg(color(&scene.background));
    buffer.set_style(area, base);
    for component in &scene.components {
        let rect = match component {
            Component::Panel { rect, .. }
            | Component::ListRow { rect, .. }
            | Component::Tab { rect, .. }
            | Component::Meter { rect, .. }
            | Component::Image { rect, .. }
            | Component::Menu { rect }
            | Component::Dialog { rect, .. }
            | Component::TextField { rect, .. }
            | Component::Terminal { rect, .. } => {
                Rect::new(rect.x, rect.y, rect.width, rect.height)
            }
        }
        .intersection(area);
        if rect.is_empty() {
            continue;
        }
        match component {
            Component::Panel { active, .. } => {
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(base.fg(color(if *active {
                        &scene.accent
                    } else {
                        &scene.border
                    })))
                    .render(rect, &mut buffer);
            }
            Component::Menu { .. } | Component::Dialog { .. } => {
                Block::default()
                    .borders(Borders::ALL)
                    .style(base)
                    .render(rect, &mut buffer);
            }
            Component::ListRow {
                label,
                marked,
                foreground,
                background,
                ..
            } => {
                let style = base.fg(color(foreground)).bg(color(background));
                buffer.set_style(rect, style);
                text(
                    &mut buffer,
                    rect.x,
                    rect.y,
                    &format!("{} {label}", if *marked { '●' } else { ' ' }),
                    rect.width,
                    style,
                );
            }
            Component::Tab {
                label,
                active,
                close,
                ..
            } => {
                let style = if *active {
                    base.bg(color(&scene.border))
                } else {
                    base
                };
                buffer.set_style(rect, style);
                text(&mut buffer, rect.x, rect.y, label, rect.width, style);
                if let Some(close) = close {
                    text(&mut buffer, close.x, close.y, "×", close.width, style);
                }
            }
            Component::TextField {
                text: value,
                secret,
                ..
            } => {
                let value = if *secret {
                    "•".repeat(value.chars().count().min(128))
                } else {
                    value.clone()
                };
                text(&mut buffer, rect.x, rect.y, &value, rect.width, base);
            }
            Component::Meter {
                value,
                foreground,
                background,
                ..
            } => {
                let filled = u32::from(rect.width) * u32::from((*value).min(1000)) / 1000;
                for x in rect.x..rect.right() {
                    buffer[(x, rect.y)].set_symbol("■").set_fg(color(
                        if u32::from(x - rect.x) < filled {
                            foreground
                        } else {
                            background
                        },
                    ));
                }
            }
            Component::Image { .. } | Component::Terminal { .. } => {}
        }
    }
    // Application compatibility spans are the authoritative cell view,
    // including established menus, player/editor cells and operation reports.
    for span in &scene.spans {
        let style = base
            .fg(color(&span.foreground))
            .bg(color(&span.background))
            .add_modifier(if span.bold {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        text(
            &mut buffer,
            span.x,
            span.y,
            &span.text,
            area.width.saturating_sub(span.x),
            style,
        );
    }
    buffer
}

pub struct Cells {
    previous: Option<Buffer>,
    output: Receiver<RenderMessage>,
    frames: Sender<RenderMessage>,
}
impl Default for Cells {
    fn default() -> Self {
        let (frames, output) = bounded(1);
        Self {
            previous: None,
            output,
            frames,
        }
    }
}
impl Cells {
    pub fn output(&self) -> &Receiver<RenderMessage> {
        &self.output
    }
    pub fn scene(&mut self, scene: &Scene) -> Result<()> {
        let next = buffer(scene);
        let mut out = io::stdout().lock();
        out.write_all(b"\x1b[?2026h")?;
        if self
            .previous
            .as_ref()
            .is_none_or(|old| old.area != next.area)
        {
            out.write_all(b"\x1b[2J\x1b[H")?;
            self.previous = Some(Buffer::empty(next.area));
        }
        let old = self.previous.as_ref().unwrap();
        let mut backend = CrosstermBackend::new(&mut out);
        backend.draw(old.diff(&next).into_iter())?;
        Backend::flush(&mut backend)?;
        out.write_all(b"\x1b[?2026l")?;
        out.flush()?;
        self.previous = Some(next);
        // Cell drawing is synchronous: acknowledgements follow the actual
        // terminal write, rather than a browser paint notification.
        let _ = self.output.try_recv();
        self.frames.try_send(RenderMessage::Frame {
            revision: scene.revision,
            generation: scene.viewport.generation,
            width: scene.viewport.width,
            height: scene.viewport.height,
            png: String::new(),
        })?;
        Ok(())
    }
    pub fn clipboard(&mut self, text: &str) -> Result<()> {
        let data = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let mut out = io::stdout().lock();
        write!(out, "\x1b]52;c;{data}\x1b\\")?;
        out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{Span, Viewport};
    use super::*;

    #[test]
    fn cell_view_keeps_controller_text_and_styles_without_terminal_controls() {
        let mut scene = Scene::from_buffer(
            &Buffer::empty(Rect::new(0, 0, 20, 5)),
            Viewport {
                columns: 20,
                rows: 5,
                ..Viewport::default()
            },
            1,
        );
        scene.components.push(Component::ListRow {
            rect: super::super::protocol::Rect {
                x: 1,
                y: 1,
                width: 18,
                height: 1,
            },
            label: "semantic label".into(),
            icon: "file".into(),
            foreground: "#ffffff".into(),
            background: "#111111".into(),
            selected: true,
            marked: true,
        });
        scene.spans.push(Span {
            x: 1,
            y: 1,
            text: "controller 日本語\x1b".into(),
            foreground: "#abcdef".into(),
            background: "#222222".into(),
            bold: true,
        });
        let output = buffer(&scene);
        assert_eq!(output[(1, 1)].symbol(), "c");
        assert_eq!(output[(1, 1)].fg, Color::Rgb(0xab, 0xcd, 0xef));
        assert!(output
            .content
            .iter()
            .all(|cell| !cell.symbol().contains('\x1b')));
    }

    #[test]
    fn clipped_components_and_untrusted_colors_cannot_panic() {
        let mut scene = Scene::from_buffer(
            &Buffer::empty(Rect::new(0, 0, 20, 5)),
            Viewport {
                columns: 20,
                rows: 5,
                ..Viewport::default()
            },
            1,
        );
        scene.components.push(Component::Panel {
            rect: super::super::protocol::Rect {
                x: 19,
                y: 4,
                width: 100,
                height: 100,
            },
            active: true,
        });
        scene.accent = "#1ø234".into();
        assert_eq!(buffer(&scene).area.width, 20);
        assert_eq!(color("#1ø234"), Color::Reset);
    }
}
