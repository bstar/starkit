//! PTY cell updates rendered by the host terminal, never by the image renderer.
use super::protocol::{Component, Rect, Scene, Viewport};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub columns: u16,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    /// Bold, dim, italic, underline, inverse.
    pub attributes: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub index: u16,
    pub runs: Vec<Run>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub stream: String,
    pub generation: u64,
    pub rect: Rect,
    pub complete: bool,
    pub rows: Vec<Row>,
    pub cursor: Option<[u16; 2]>,
}
impl Frame {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.stream.is_empty() && self.stream.len() <= 96,
            "Invalid terminal stream"
        );
        ensure!(
            self.rect.width > 0
                && self.rect.width <= 512
                && self.rect.height > 0
                && self.rect.height <= 192,
            "Invalid terminal grid"
        );
        ensure!(
            self.rows.len() <= usize::from(self.rect.height),
            "Too many terminal rows"
        );
        let mut seen = std::collections::HashSet::new();
        for row in &self.rows {
            ensure!(
                row.index < self.rect.height && seen.insert(row.index),
                "Invalid terminal row"
            );
            let mut columns = 0u32;
            for run in &row.runs {
                ensure!(
                    run.columns > 0
                        && run.text.len() <= usize::from(run.columns) * 64
                        && !run.text.chars().any(char::is_control),
                    "Invalid terminal text"
                );
                ensure!(
                    run.attributes & !31 == 0 && run.text.width() <= usize::from(run.columns),
                    "Invalid terminal run width"
                );
                columns += u32::from(run.columns);
            }
            ensure!(
                columns == u32::from(self.rect.width),
                "Incomplete terminal row"
            );
        }
        ensure!(
            !self.complete || self.rows.len() == usize::from(self.rect.height),
            "Incomplete initial terminal grid"
        );
        if let Some([column, row]) = self.cursor {
            ensure!(
                column < self.rect.width && row < self.rect.height,
                "Terminal cursor outside pane"
            );
        }
        Ok(())
    }
}
struct Layer {
    frame: Frame,
    rows: Vec<Row>,
    dirty: Vec<bool>,
    visible: bool,
}
#[derive(Default)]
pub(crate) struct Frontend {
    layers: HashMap<String, Layer>,
    cursor: Option<[u16; 2]>,
    cursor_dirty: bool,
}
impl Frontend {
    pub fn receive(&mut self, frame: Frame) -> Result<()> {
        frame.validate()?;
        if frame.complete {
            // Native grids cannot overlap: retire the previous tab/split.
            self.layers.retain(|_, layer| {
                let a = layer.frame.rect;
                let b = frame.rect;
                let overlap = u32::from(a.x) < u32::from(b.x) + u32::from(b.width)
                    && u32::from(b.x) < u32::from(a.x) + u32::from(a.width)
                    && u32::from(a.y) < u32::from(b.y) + u32::from(b.height)
                    && u32::from(b.y) < u32::from(a.y) + u32::from(a.height);
                layer.frame.stream == frame.stream
                    || (!overlap && layer.frame.generation == frame.generation)
            });
            ensure!(
                self.layers.contains_key(&frame.stream) || self.layers.len() < 8,
                "Too many terminal panes"
            );
            let mut rows = frame.rows.clone();
            rows.sort_by_key(|r| r.index);
            self.layers.insert(
                frame.stream.clone(),
                Layer {
                    dirty: vec![true; rows.len()],
                    rows,
                    frame,
                    visible: false,
                },
            );
        } else {
            let layer = self
                .layers
                .get_mut(&frame.stream)
                .ok_or_else(|| anyhow::anyhow!("Terminal grid base is missing"))?;
            ensure!(
                layer.frame.rect == frame.rect && layer.frame.generation == frame.generation,
                "Terminal grid base changed"
            );
            for row in &frame.rows {
                layer.rows[usize::from(row.index)] = row.clone();
                layer.dirty[usize::from(row.index)] = true;
            }
            layer.frame.cursor = frame.cursor;
        }
        Ok(())
    }
    pub fn restore_cursor(&mut self) {
        if !self.layers.is_empty() {
            self.cursor_dirty = true;
        }
    }
    pub fn invalidate(&mut self) {
        if !self.layers.is_empty() {
            self.cursor_dirty = true;
        }
        for l in self.layers.values_mut() {
            l.dirty.fill(true);
        }
    }
    pub fn present(
        &mut self,
        scene: &Scene,
        viewport: Viewport,
        out: &mut impl Write,
    ) -> Result<()> {
        if scene.viewport.generation != viewport.generation {
            return Ok(());
        }
        let mut cursor = None;
        let mut began = false;
        for layer in self.layers.values_mut() {
            let rect = layer.frame.rect;
            let visible = layer.frame.generation == viewport.generation
                && u32::from(rect.x) + u32::from(rect.width) <= u32::from(viewport.columns)
                && u32::from(rect.y) + u32::from(rect.height) <= u32::from(viewport.rows)
                && scene
                    .components
                    .iter()
                    .any(|c| matches!(c, Component::Terminal { rect: r } if *r == rect));
            if !visible {
                layer.visible = false;
                continue;
            }
            if !layer.visible {
                layer.dirty.fill(true);
                layer.visible = true;
            }
            for (row, dirty) in layer.rows.iter().zip(layer.dirty.iter_mut()) {
                if !*dirty {
                    continue;
                }
                if !began {
                    out.write_all(b"\x1b[?2026h\x1b[?25l\x1b[?7l")?;
                    began = true;
                }
                write!(out, "\x1b[{};{}H", rect.y + row.index + 1, rect.x + 1)?;
                for run in &row.runs {
                    let [r, g, b] = run.foreground;
                    let [br, bg, bb] = run.background;
                    write!(out, "\x1b[0;38;2;{r};{g};{b};48;2;{br};{bg};{bb}m")?;
                    for (bit, sgr) in [(1, 1), (2, 2), (4, 3), (8, 4), (16, 7)] {
                        if run.attributes & bit != 0 {
                            write!(out, "\x1b[{sgr}m")?;
                        }
                    }
                    out.write_all(run.text.as_bytes())?;
                    let padding = usize::from(run.columns).saturating_sub(run.text.width());
                    if padding > 0 {
                        out.write_all(&vec![b' '; padding])?;
                    }
                }
                *dirty = false;
            }
            if let Some([x, y]) = layer.frame.cursor {
                cursor = Some([rect.x + x, rect.y + y]);
            }
        }
        if began || self.cursor != cursor || self.cursor_dirty {
            out.write_all(b"\x1b[0m\x1b[?7h")?;
            if let Some([x, y]) = cursor {
                write!(out, "\x1b[{};{}H\x1b[?25h", y + 1, x + 1)?;
            } else {
                out.write_all(b"\x1b[?25l")?;
            }
            if began {
                out.write_all(b"\x1b[?2026l")?;
            }
            out.flush()?;
            self.cursor = cursor;
            self.cursor_dirty = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> Frame {
        Frame {
            stream: "pty-1".into(),
            generation: 1,
            rect: Rect {
                x: 3,
                y: 2,
                width: 8,
                height: 2,
            },
            complete: true,
            rows: (0..2)
                .map(|index| Row {
                    index,
                    runs: vec![Run {
                        text: "Hi 世界 ".into(),
                        columns: 8,
                        foreground: [220, 210, 190],
                        background: [31, 31, 40],
                        attributes: 5,
                    }],
                })
                .collect(),
            cursor: Some([2, 1]),
        }
    }
    fn scene(frame: &Frame) -> Scene {
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(0, 0, 80, 24)),
            Viewport {
                columns: 80,
                rows: 24,
                width: 800,
                height: 480,
                generation: 1,
            },
            1,
        );
        scene
            .components
            .push(Component::Terminal { rect: frame.rect });
        scene
    }
    #[test]
    fn apps_without_native_panes_have_no_terminal_side_effects() {
        let mut frontend = Frontend::default();
        let mut scene = scene(&frame());
        scene.components.clear();
        frontend.restore_cursor();
        frontend.invalidate();
        let mut wire = vec![];
        frontend.present(&scene, scene.viewport, &mut wire).unwrap();
        assert!(wire.is_empty());
    }
    #[test]
    fn switching_tabs_and_split_geometry_retires_overlapping_grids() {
        let mut frontend = Frontend::default();
        for index in 0..32 {
            let mut next = frame();
            next.stream = format!("pane-{index}");
            next.rect.x += index;
            frontend.receive(next).unwrap();
            assert_eq!(frontend.layers.len(), 1);
        }
    }
    #[test]
    fn native_rows_emit_text_sgr_and_cursor_without_an_image() {
        let frame = frame();
        frame.validate().unwrap();
        let scene = scene(&frame);
        let mut frontend = Frontend::default();
        frontend.receive(frame).unwrap();
        let mut wire = vec![];
        frontend.present(&scene, scene.viewport, &mut wire).unwrap();
        let wire = String::from_utf8(wire).unwrap();
        assert!(wire.contains("\x1b[3;4H"));
        assert!(wire.contains("Hi 世界 "));
        assert!(wire.contains("\x1b[4;6H\x1b[?25h"));
        assert!(!wire.contains("\x1b_G"));
        let mut wire = vec![];
        frontend.present(&scene, scene.viewport, &mut wire).unwrap();
        assert!(
            wire.is_empty(),
            "Idle terminals must not keep moving the cursor or resetting its blink"
        );
    }
    #[test]
    fn changed_row_is_native_text_and_hidden_panes_do_not_write() {
        let first = frame();
        let mut scene = scene(&first);
        let mut frontend = Frontend::default();
        frontend.receive(first.clone()).unwrap();
        frontend
            .present(&scene, scene.viewport, &mut vec![])
            .unwrap();
        let mut update = first;
        update.complete = false;
        update.rows.truncate(1);
        update.rows[0].runs[0].text = "Changed ".into();
        frontend.receive(update).unwrap();
        let mut wire = vec![];
        frontend.present(&scene, scene.viewport, &mut wire).unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(text.contains("Changed "));
        assert!(!text.contains("世界"));
        scene.components.clear();
        let mut wire = vec![];
        frontend.present(&scene, scene.viewport, &mut wire).unwrap();
        assert_eq!(wire, b"\x1b[0m\x1b[?7h\x1b[?25l");
    }
    #[test]
    fn unsafe_text_and_invalid_cell_bounds_are_rejected() {
        let mut bad = frame();
        bad.rows[0].runs[0].text = "\x1b]52;c;secret\x07".into();
        assert!(bad.validate().is_err());
        let mut bad = frame();
        bad.rows[0].runs[0].columns = 1;
        assert!(bad.validate().is_err());
        let mut bad = frame();
        bad.rows[1].index = 0;
        assert!(bad.validate().is_err());
        let mut bad = frame();
        bad.cursor = Some([8, 0]);
        assert!(bad.validate().is_err());
    }
    #[cfg(feature = "gpu-renderer")]
    #[test]
    #[ignore = "Requires a hardware GPU; renders an offscreen texture only"]
    fn native_terminal_component_has_transparent_holes_on_gpu() {
        let scene = scene(&frame());
        let mut renderer =
            super::super::renderer::SceneRenderer::new(super::super::renderer::Backend::Gpu)
                .unwrap();
        let image = renderer.render(&scene).unwrap();
        eprintln!("native terminal GPU renderer: {:?}", renderer.backend());
        assert_eq!(image.get_pixel(35, 45)[3], 0);
        assert_eq!(image.get_pixel(5, 5)[3], 255);
    }
    #[test]
    fn native_terminal_component_punches_a_hole_in_graphical_chrome() {
        let frame = frame();
        let scene = scene(&frame);
        let mut renderer =
            super::super::renderer::SceneRenderer::new(super::super::renderer::Backend::Cpu)
                .unwrap();
        let image = renderer.render(&scene).unwrap();
        assert_eq!(image.get_pixel(35, 45)[3], 0);
        assert_eq!(image.get_pixel(5, 5)[3], 255);
    }
}
