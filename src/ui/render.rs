use super::{Frame, Presentation, Runtime, Viewport, Widget};
use crate::{
    chrome::{
        frame::{self, Frame as ChromeFrame, NO_WORDS},
        rgb,
    },
    native_surface::{PixelRect, Surface},
    ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Modifier, Style},
    },
};

fn pane_focused(frame: &Frame, p: &super::Placement) -> bool {
    (p.modal && p.parent.is_none())
        || frame
            .focus
            .as_ref()
            .is_some_and(|id| frame.ancestry(id).contains(&p.id))
}

fn pixel_rect(area: Rect) -> PixelRect {
    PixelRect::new(area.x, area.y, area.width, area.height)
}

fn line(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    if area.is_empty() {
        return;
    }
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    buf.set_stringn(area.x, area.y, text, usize::from(area.width), style);
}
fn field_window(value: &str, cursor: usize, secret: bool) -> (String, usize) {
    let mut start = cursor.saturating_sub(2048);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (cursor + 2048).min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    if secret {
        (
            "*".repeat(value[start..end].chars().count()),
            value[start..cursor].chars().count(),
        )
    } else {
        (value[start..end].to_string(), cursor - start)
    }
}

fn cell_column(
    buf: &mut Buffer,
    column: &super::ColumnPlacement,
    y: u16,
    text: &str,
    style: Style,
) {
    if column.area.width == 0 {
        return;
    }
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    let text = if crate::wrap::width_of(&clean) <= column.area.width {
        clean
    } else {
        let mut clipped = String::new();
        let mut used: u16 = 0;
        for (_, cluster) in crate::wrap::clusters(&clean) {
            let width = crate::wrap::width_of(cluster);
            if used.saturating_add(width) > column.area.width - 1 {
                break;
            }
            clipped.push_str(cluster);
            used += width;
        }
        clipped.push('…');
        clipped
    };
    let spare = column
        .area
        .width
        .saturating_sub(crate::wrap::width_of(&text));
    let offset = match column.align {
        super::Align::Left => 0,
        super::Align::Center => spare / 2,
        super::Align::Right => spare,
    };
    line(
        buf,
        Rect::new(column.area.x + offset, y, column.area.width - offset, 1),
        &text,
        style,
    );
}
fn native_column(
    surface: &mut Surface,
    column: &super::ColumnPlacement,
    y: u16,
    height: u16,
    text: String,
    ink: &str,
    bold: bool,
) {
    if column.area.width == 0 || height == 0 {
        return;
    }
    let rect = PixelRect::new(column.area.x, y, column.area.width, height);
    match column.align {
        super::Align::Left => surface.text(rect, text, ink, 12, bold),
        super::Align::Center => surface.text_centered(rect, text, ink, 12, bold),
        super::Align::Right => surface.text_right(rect, text, ink, 12, bold),
    }
}
fn item_column(item: &super::Item, column: usize, native: bool) -> String {
    let value = item.columns.get(column).map_or("", String::as_str);
    if column == 0 {
        format!(
            "{}{}{}",
            " ".repeat(usize::from(item.depth.min(16)) * 2),
            if item.checked {
                if native {
                    "✓ "
                } else {
                    "[x] "
                }
            } else {
                ""
            },
            value
        )
    } else {
        value.into()
    }
}
impl Runtime {
    pub fn render_cells(&mut self, viewport: Viewport) -> anyhow::Result<Buffer> {
        self.render_cells_with(viewport, &mut super::content::EmptyContent)
    }
    pub fn render_cells_with(
        &mut self,
        viewport: Viewport,
        content: &mut impl super::content::Content,
    ) -> anyhow::Result<Buffer> {
        anyhow::ensure!(
            viewport.presentation == Presentation::Cells,
            "Cell renderer requires a cell viewport"
        );
        let frame = self.frame(viewport)?.clone();
        let mut buf = Buffer::empty(Rect::new(0, 0, viewport.width, viewport.height));
        buf.set_style(
            buf.area,
            Style::default()
                .fg(rgb(self.theme.fg))
                .bg(rgb(self.theme.bg)),
        );
        // Explicitly disable the legacy thread-local graphical padding adapter.
        let _scope = frame::padding_scope(false);
        for p in &frame.placements {
            let area = p.area;
            let focused = frame.focus.as_ref() == Some(&p.id);
            let theme = &self.theme;
            let style = Style::default()
                .fg(rgb(if p.enabled { theme.fg } else { theme.dim }))
                .bg(rgb(theme.panel_bg));
            if matches!(p.widget, Widget::Custom { .. }) {
                let mut local = Buffer::empty(Rect::new(0, 0, p.body.width, p.body.height));
                local.set_style(local.area, style);
                if content.cells(&p.id, &mut local) {
                    anyhow::ensure!(
                        local.area == Rect::new(0, 0, p.body.width, p.body.height)
                            && local.content.len()
                                == usize::from(p.body.width) * usize::from(p.body.height),
                        "Custom cell view changed its bounds"
                    );
                    for y in 0..p.body.height {
                        for x in 0..p.body.width {
                            buf[(p.body.x + x, p.body.y + y)] = local[(x, y)].clone();
                        }
                    }
                    continue;
                }
            }
            match &p.widget {
                Widget::Row | Widget::Column => {}
                Widget::Pane(heading) => {
                    frame::frame(
                        area,
                        &mut buf,
                        &ChromeFrame {
                            theme,
                            focused: pane_focused(&frame, p),
                            title: &heading.title,
                            detail: None,
                            heading: false,
                            badge: (!heading.status.is_empty()).then_some(
                                crate::chrome::frame::Badge {
                                    text: &heading.status,
                                    tone: crate::chrome::frame::Tone::Dim,
                                },
                            ),
                            footer: None,
                            words: NO_WORDS,
                        },
                    );
                }
                Widget::Label(text) | Widget::Custom { fallback: text, .. } => {
                    for (row, text) in text.lines().take(usize::from(area.height)).enumerate() {
                        line(
                            &mut buf,
                            Rect::new(area.x, area.y + row as u16, area.width, 1),
                            text,
                            style,
                        );
                    }
                }
                Widget::Status(text) => {
                    buf.set_style(area, style.bg(rgb(theme.status_bg)));
                    line(
                        &mut buf,
                        area,
                        text,
                        style.fg(rgb(theme.status_fg)).bg(rgb(theme.status_bg)),
                    );
                }
                Widget::Button { command } | Widget::Checkbox { command } => {
                    if let Some(command) = self.registry.get(command) {
                        let label = if matches!(p.widget, Widget::Checkbox { .. }) {
                            format!(
                                "[{}] {}",
                                if command.checked { 'x' } else { ' ' },
                                command.label
                            )
                        } else {
                            format!("[ {} ]", command.label)
                        };
                        let style = if focused {
                            style
                                .fg(rgb(theme.row_cursor_fg))
                                .bg(rgb(theme.row_cursor_bg))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            style
                        };
                        buf.set_style(area, style);
                        line(&mut buf, area, &label, style);
                    }
                }
                Widget::Field {
                    value,
                    placeholder,
                    secret,
                    ..
                } => {
                    let style = style.bg(rgb(theme.row_bg)).add_modifier(if focused {
                        Modifier::UNDERLINED
                    } else {
                        Modifier::empty()
                    });
                    buf.set_style(area, style);
                    if value.is_empty() && !focused {
                        line(&mut buf, area, placeholder, style.fg(rgb(theme.dim)));
                    } else if let Some(field) = self.fields.get_mut(&p.id) {
                        // Use the editor's scrolling/caret rules for passwords
                        // too, without ever placing plaintext in the buffer.
                        let mut masked;
                        let field = if *secret {
                            masked = field.clone();
                            let cursor = field.text()[..field.cursor()].chars().count();
                            masked.set_text(
                                field
                                    .text()
                                    .chars()
                                    .map(|c| if c == '\n' { '\n' } else { '*' })
                                    .collect::<String>(),
                            );
                            masked.set_cursor(cursor);
                            &mut masked
                        } else {
                            field
                        };
                        if let Some((x, y)) =
                            field.render(area, &mut buf, style).filter(|_| focused)
                        {
                            if buf.area.contains((x, y).into()) {
                                buf[(x, y)].set_style(style.add_modifier(Modifier::REVERSED));
                            }
                        }
                    }
                }
                Widget::List {
                    items,
                    selected,
                    columns,
                } => {
                    let header = usize::from(!columns.is_empty());
                    let rows = usize::from(area.height).saturating_sub(header);
                    let offset = self.scroll.get(&p.id).copied().unwrap_or(0);
                    if header > 0 {
                        for (column, heading) in p.columns.iter().zip(columns) {
                            cell_column(
                                &mut buf,
                                column,
                                area.y,
                                heading,
                                style.fg(rgb(theme.header_fg)).add_modifier(Modifier::BOLD),
                            );
                        }
                    }
                    for (row, item) in items.iter().skip(offset).take(rows).enumerate() {
                        let rect = Rect::new(area.x, area.y + (row + header) as u16, area.width, 1);
                        let selected = selected.as_ref() == Some(&item.id);
                        let style = if selected {
                            style
                                .fg(rgb(theme.row_selected_fg))
                                .bg(rgb(theme.row_selected_bg))
                                .add_modifier(
                                    Modifier::REVERSED
                                        & if focused {
                                            Modifier::REVERSED
                                        } else {
                                            Modifier::empty()
                                        },
                                )
                        } else if !item.enabled {
                            style.fg(rgb(theme.dim))
                        } else {
                            style
                        };
                        buf.set_style(rect, style);
                        if header > 0 {
                            for (index, column) in p.columns.iter().enumerate() {
                                cell_column(
                                    &mut buf,
                                    column,
                                    rect.y,
                                    &item_column(item, index, false),
                                    style,
                                );
                            }
                        } else {
                            line(
                                &mut buf,
                                rect,
                                &format!(
                                    "{}{}{}",
                                    " ".repeat(usize::from(item.depth.min(16)) * 2),
                                    if item.checked { "[x] " } else { "" },
                                    item.columns.join("  ")
                                ),
                                style,
                            );
                        }
                    }
                }
                Widget::Slider { value, max, .. } | Widget::Meter { value, max } => {
                    let filled = (u32::from(area.width) * u32::from(*value)
                        / u32::from((*max).max(1))) as u16;
                    for x in 0..area.width {
                        let glyph = if matches!(p.widget, Widget::Slider { .. })
                            && x == filled.min(area.width.saturating_sub(1))
                        {
                            "|"
                        } else if x < filled {
                            "█"
                        } else {
                            "─"
                        };
                        buf[(area.x + x, area.y)].set_symbol(glyph).set_style(
                            style.fg(rgb(if focused { theme.accent } else { theme.dim })),
                        );
                    }
                }
                Widget::Separator => line(
                    &mut buf,
                    area,
                    &"─".repeat(usize::from(area.width)),
                    style.fg(rgb(theme.divider)),
                ),
            }
        }
        Ok(buf)
    }

    /// Native controls use physical text and the same retained component geometry
    /// as pointer routing. The surface is reusable inside the SSH scene protocol.
    pub fn render_surface(&mut self, viewport: Viewport) -> anyhow::Result<Surface> {
        anyhow::ensure!(
            viewport.presentation == Presentation::Graphics,
            "Native renderer requires a pixel viewport"
        );
        let frame = self.frame(viewport)?.clone();
        let mut surface = self
            .visual
            .surface(viewport.width.max(1), viewport.height.max(1))
            .with_ui_text();
        for p in &frame.placements {
            self.paint_native(&mut surface, &frame, p);
        }
        surface.validate()?;
        Ok(surface)
    }
    fn paint_native(&mut self, surface: &mut Surface, frame: &Frame, p: &super::Placement) {
        let node_start = surface.nodes.len();
        let hit_start = surface.hits.len();
        let hover_start = surface.hover_fills.len();
        let style = &self.visual;
        let rect = PixelRect::new(p.area.x, p.area.y, p.area.width, p.area.height);
        let focused = frame.focus.as_ref() == Some(&p.id);
        let ink = if p.enabled {
            &style.colors.ink
        } else {
            &style.colors.dim
        };
        let action = format!("kit.ui:{}", p.id.0);
        match &p.widget {
            Widget::Row | Widget::Column => {}
            Widget::Pane(heading) => {
                style.frame(surface, rect, false);
                if rect.height >= super::layout::BODY_TOP && rect.width >= 16 {
                    let header = super::layout::pane_header(p.area);
                    surface.fill(pixel_rect(header), &style.colors.title, 0);
                    // Align headings with field, label and list text below.
                    let body_text = super::layout::text_area(p.body);
                    let text = Rect::new(body_text.x, header.y, body_text.width, header.height);
                    let status_width = if heading.status.is_empty() {
                        0
                    } else {
                        (heading.status.chars().count() as u16)
                            .saturating_mul(8)
                            .min(text.width / 2)
                    };
                    surface.text(
                        PixelRect::new(
                            text.x,
                            text.y,
                            text.width.saturating_sub(status_width),
                            text.height,
                        ),
                        &heading.title,
                        &style.colors.ink,
                        12,
                        true,
                    );
                    if status_width > 0 {
                        surface.text(
                            PixelRect::new(
                                text.right() - status_width,
                                text.y,
                                status_width,
                                text.height,
                            ),
                            &heading.status,
                            &style.colors.dim,
                            11,
                            false,
                        );
                    }
                }
            }
            Widget::Label(text) | Widget::Custom { fallback: text, .. } => {
                for (row, text) in text.lines().take(usize::from(rect.height / 24)).enumerate() {
                    surface.text(
                        PixelRect::new(
                            rect.x + 4.min(rect.width),
                            rect.y + row as u16 * 24,
                            rect.width.saturating_sub(8),
                            24,
                        ),
                        text,
                        ink,
                        12,
                        false,
                    );
                }
            }
            Widget::Status(text) => {
                surface.fill(rect, &style.colors.title, 0);
                surface.text(
                    pixel_rect(super::layout::text_area(p.area)),
                    text,
                    ink,
                    12,
                    false,
                );
            }
            Widget::Button { command } | Widget::Checkbox { command } => {
                if let Some(command) = self.registry.get(command) {
                    let text = if matches!(p.widget, Widget::Checkbox { .. }) {
                        format!(
                            "[{}] {}",
                            if command.checked { 'x' } else { ' ' },
                            command.label
                        )
                    } else {
                        command.label.clone()
                    };
                    if p.enabled {
                        style.button(surface, rect, &text, &action, command.checked);
                    } else {
                        style.frame(surface, rect, true);
                        surface.text_centered(rect, text, ink, 12, false);
                    }
                }
            }
            Widget::Field {
                value,
                placeholder,
                secret,
                multiline,
            } => {
                style.frame(surface, rect, true);
                let cursor = self
                    .fields
                    .get(&p.id)
                    .map_or(value.len(), |field| field.cursor());
                let lines: Vec<_> = if *multiline {
                    value.split('\n').collect()
                } else {
                    vec![value.as_str()]
                };
                let cursor_row = if *multiline {
                    value[..cursor].bytes().filter(|b| *b == b'\n').count()
                } else {
                    0
                };
                let visible = usize::from((rect.height / 24).max(1));
                let first = cursor_row.saturating_sub(visible - 1);
                let mut byte_base = lines
                    .iter()
                    .take(first)
                    .map(|line| line.len() + 1)
                    .sum::<usize>();
                for (row, line) in lines.iter().skip(first).take(visible).enumerate() {
                    let is_caret_line = row + first == cursor_row && focused;
                    let local_cursor = if is_caret_line {
                        cursor.saturating_sub(byte_base).min(line.len())
                    } else {
                        0
                    };
                    let (text, caret) = field_window(line, local_cursor, *secret);
                    let text = if value.is_empty() && !focused {
                        placeholder.clone()
                    } else {
                        text
                    };
                    let node = surface.nodes.len() as u16;
                    let top = rect.y + row as u16 * 24;
                    let height = 24.min(rect.y + rect.height - top);
                    surface.text(
                        PixelRect::new(
                            rect.x + 4.min(rect.width),
                            top,
                            rect.width.saturating_sub(8),
                            height,
                        ),
                        text,
                        ink,
                        12,
                        false,
                    );
                    if is_caret_line {
                        surface.text_carets.push(crate::native_surface::TextCaret {
                            node,
                            byte: caret,
                            color: style.colors.accent.clone(),
                        });
                    }
                    byte_base += line.len() + 1;
                }
            }
            Widget::List {
                items,
                selected,
                columns,
            } => {
                let height = 24u16;
                let header = u16::from(!columns.is_empty());
                let rows = rect.height / height;
                if header > 0 {
                    for (column, heading) in p.columns.iter().zip(columns) {
                        native_column(
                            surface,
                            column,
                            rect.y,
                            height.min(rect.height),
                            heading.clone(),
                            ink,
                            true,
                        );
                    }
                }
                let offset = self.scroll.get(&p.id).copied().unwrap_or(0);
                for (row, item) in items
                    .iter()
                    .skip(offset)
                    .take(usize::from(rows.saturating_sub(header)))
                    .enumerate()
                {
                    let row = PixelRect::new(
                        rect.x,
                        rect.y + (row as u16 + header) * height,
                        rect.width,
                        height,
                    );
                    if selected.as_ref() == Some(&item.id) {
                        surface.fill(
                            row,
                            &crate::native_surface::style::hex(style.palette.selected),
                            0,
                        );
                    }
                    if header > 0 {
                        for (index, column) in p.columns.iter().enumerate() {
                            native_column(
                                surface,
                                column,
                                row.y,
                                height,
                                item_column(item, index, true),
                                if item.enabled { ink } else { &style.colors.dim },
                                false,
                            );
                        }
                    } else {
                        surface.text(
                            PixelRect::new(
                                row.x + 4,
                                row.y,
                                row.width.saturating_sub(8),
                                row.height,
                            ),
                            format!(
                                "{}{}{}",
                                " ".repeat(usize::from(item.depth.min(16)) * 2),
                                if item.checked { "✓ " } else { "" },
                                item.columns.join("  ")
                            ),
                            if item.enabled { ink } else { &style.colors.dim },
                            12,
                            false,
                        );
                    }
                }
            }
            Widget::Slider { value, max, .. } | Widget::Meter { value, max } => {
                let filled =
                    (u32::from(rect.width) * u32::from(*value) / u32::from((*max).max(1))) as u16;
                let height = rect.height.min(6);
                let y = rect.y + (rect.height - height) / 2;
                surface.fill(
                    PixelRect::new(rect.x, y, rect.width, height),
                    &style.colors.inset,
                    0,
                );
                surface.fill(
                    PixelRect::new(rect.x, y, filled, height),
                    &style.colors.accent,
                    0,
                );
                if matches!(p.widget, Widget::Slider { .. }) {
                    let width = rect.width.min(6);
                    surface.fill(
                        PixelRect::new(
                            rect.x + filled.saturating_sub(width / 2).min(rect.width - width),
                            rect.y,
                            width,
                            rect.height,
                        ),
                        &style.colors.raised,
                        0,
                    );
                }
            }
            Widget::Separator => surface.fill(
                PixelRect::new(rect.x, rect.y + rect.height / 2, rect.width, 1),
                &style.colors.shadow,
                0,
            ),
        }
        for node in &mut surface.nodes[node_start..] {
            if let crate::native_surface::Primitive::Text { text, .. } = node {
                if text.len() > 8192 {
                    let mut end = 8192;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                }
            }
        }
        if (focused || matches!(p.widget, Widget::Pane(_)) && pane_focused(frame, p))
            && rect.width > 2
            && rect.height > 2
        {
            surface
                .nodes
                .push(crate::native_surface::Primitive::Border {
                    rect,
                    color: style.colors.accent.clone(),
                    radius: 0,
                });
        }
        if p.enabled && p.focusable && !surface.hits.iter().any(|h| h.action == action) {
            surface
                .hits
                .push(crate::native_surface::HitRegion { rect, action });
        }
        if !p.modal && frame.placements.iter().any(|p| p.modal) {
            surface.hits.truncate(hit_start);
            surface.hover_fills.truncate(hover_start);
        }
    }
    #[cfg(feature = "terminal-graphics")]
    pub fn render_scene(
        &mut self,
        viewport: crate::terminal_graphics::Viewport,
        cells: bool,
    ) -> anyhow::Result<crate::terminal_graphics::Scene> {
        self.render_scene_with(viewport, cells, &mut super::content::EmptyContent)
    }
    #[cfg(feature = "terminal-graphics")]
    pub fn render_scene_with(
        &mut self,
        viewport: crate::terminal_graphics::Viewport,
        cells: bool,
        content: &mut impl super::content::Content,
    ) -> anyhow::Result<crate::terminal_graphics::Scene> {
        viewport.validate()?;
        let dimensions = if cells {
            Viewport {
                generation: viewport.generation,
                ..Viewport::cells(viewport.columns, viewport.rows)
            }
        } else {
            Viewport {
                generation: viewport.generation,
                ..Viewport::graphics(viewport.width as u16, viewport.height as u16)
            }
        };
        let mut scene = if cells {
            let buffer = self.render_cells_with(dimensions, content)?;
            crate::terminal_graphics::Scene::from_buffer(
                &buffer,
                viewport,
                self.frame(dimensions)?.revision,
            )
        } else {
            let frame = self.frame(dimensions)?.clone();
            let mut scene = self.visual.scene(viewport);
            for modal in [false, true] {
                let entries: Vec<_> = frame
                    .placements
                    .iter()
                    .filter(|p| p.modal == modal)
                    .collect();
                if entries.is_empty() {
                    continue;
                }
                let bounds = if modal {
                    entries[0].area
                } else {
                    Rect::new(0, 0, dimensions.width, dimensions.height)
                };
                let mut surface = self
                    .visual
                    .surface(bounds.width, bounds.height)
                    .with_ui_text();
                if modal {
                    scene
                        .components
                        .push(crate::terminal_graphics::Component::Dialog {
                            rect: crate::native_surface::style::logical_rect(
                                PixelRect::new(bounds.x, bounds.y, bounds.width, bounds.height),
                                viewport,
                            ),
                            title: match &entries[0].widget {
                                Widget::Pane(heading) => heading.title.clone(),
                                _ => String::new(),
                            },
                        });
                }
                let mut custom = vec![];
                for entry in entries {
                    let mut local = entry.clone();
                    local.area.x -= bounds.x;
                    local.area.y -= bounds.y;
                    local.body.x -= bounds.x;
                    local.body.y -= bounds.y;
                    for column in &mut local.columns {
                        column.area.x -= bounds.x;
                        column.area.y -= bounds.y;
                    }
                    if matches!(entry.widget, Widget::Custom { .. }) {
                        if let Some(canvas) = content.canvas(&entry.id) {
                            custom.push((
                                entry.body,
                                canvas.component(
                                    PixelRect::new(
                                        entry.body.x,
                                        entry.body.y,
                                        entry.body.width,
                                        entry.body.height,
                                    ),
                                    viewport,
                                )?,
                            ));
                            continue;
                        }
                        if let Some(surface) =
                            content.native(&entry.id, [entry.body.width, entry.body.height])
                        {
                            surface.validate()?;
                            anyhow::ensure!(
                                (surface.width, surface.height)
                                    == (entry.body.width, entry.body.height),
                                "Custom native view changed its bounds"
                            );
                            custom.push((
                                entry.body,
                                crate::terminal_graphics::Component::Surface {
                                    rect: crate::native_surface::style::logical_rect(
                                        PixelRect::new(
                                            entry.body.x,
                                            entry.body.y,
                                            entry.body.width,
                                            entry.body.height,
                                        ),
                                        viewport,
                                    ),
                                    surface,
                                },
                            ));
                            continue;
                        }
                    }
                    self.paint_native(&mut surface, &frame, &local);
                }
                self.visual.mount(&mut scene, surface, bounds.x, bounds.y)?;
                for (body, component) in custom {
                    if let crate::terminal_graphics::Component::Surface { surface, rect } =
                        component
                    {
                        let _ = rect;
                        self.visual.mount(&mut scene, surface, body.x, body.y)?;
                    } else {
                        scene.components.push(component);
                    }
                }
            }
            anyhow::ensure!(
                scene.placements.len() <= 16,
                "UI exceeds native layer limit"
            );
            scene
        };
        let frame = self.frame(dimensions)?.clone();
        scene.revision = frame.revision;
        use std::hash::{Hash, Hasher};
        let mut targets = std::collections::hash_map::DefaultHasher::new();
        for p in &frame.placements {
            if p.focusable {
                (
                    &p.id,
                    p.area.x,
                    p.area.y,
                    p.area.width,
                    p.area.height,
                    p.enabled,
                    p.modal,
                )
                    .hash(&mut targets);
                match &p.widget {
                    Widget::Button { command } | Widget::Checkbox { command } => {
                        command.hash(&mut targets)
                    }
                    Widget::List { items, .. } => {
                        self.scroll
                            .get(&p.id)
                            .copied()
                            .unwrap_or(0)
                            .hash(&mut targets);
                        for item in items.iter() {
                            item.id.hash(&mut targets);
                            item.enabled.hash(&mut targets);
                        }
                    }
                    Widget::Custom { revision, .. } => revision.hash(&mut targets),
                    Widget::Slider { max, step, .. } => (max, step).hash(&mut targets),
                    _ => {}
                }
            }
        }
        scene.interaction = targets.finish();
        let mut scroll_targets = std::collections::hash_map::DefaultHasher::new();
        for p in &frame.placements {
            if matches!(p.widget, Widget::List { .. } | Widget::Custom { .. }) {
                (
                    &p.id,
                    p.area.x,
                    p.area.y,
                    p.area.width,
                    p.area.height,
                    p.enabled,
                    p.modal,
                )
                    .hash(&mut scroll_targets);
            }
        }
        scene.scroll_interaction = Some(scroll_targets.finish());
        Ok(scene)
    }
}
