//! Reusable theme chooser. KIT owns filtering, navigation and presentation;
//! applications apply and persist the selected id. Moving the highlight never
//! changes a theme, so Escape is always a cancellation.
use super::{Registry, Resolve, Theme};
use crate::{
    input::{Edit, TextInput},
    native_surface::{style::Style, PixelRect as R, Surface},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect, style::Style as CellStyle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Consumed,
    Ignored,
    Cancel,
    Select(String),
}

struct Entry {
    id: String,
    name: String,
    source: String,
    palette: crate::native_surface::style::Palette,
}

/// Built-ins, desktop and application-local themes from any KIT registry.
/// Construct on opening to pick up newly imported files. It never saves files
/// or owns an application's modal lifecycle.
pub struct Picker {
    entries: Vec<Entry>,
    current: String,
    query: TextInput,
    selected: usize,
}
impl Picker {
    pub fn new<T: Resolve>(registry: &Registry<T>, current: &str) -> Self {
        let entries: Vec<_> = registry
            .selectable()
            .into_iter()
            .map(|id| {
                let (theme, source) = registry.resolve_named(&id);
                let name = if id == "system" {
                    "System theme".into()
                } else {
                    theme.core().name.clone()
                };
                let (palette, _) = crate::native_surface::style::Palette::from_named(registry, &id);
                Entry {
                    id,
                    name,
                    source,
                    palette,
                }
            })
            .collect();
        let selected = entries.iter().position(|e| e.id == current).unwrap_or(0);
        Self {
            entries,
            current: current.into(),
            query: TextInput::single().with_max_chars(128),
            selected,
        }
    }
    /// Alt+T is the standard opener; callers reserve it even during embedded
    /// keyboard capture. No other Alt/Ctrl combinations are intercepted.
    pub fn is_shortcut(code: &str, modifiers: u8) -> bool {
        code.eq_ignore_ascii_case("char:t") && modifiers & 6 == 4
    }
    fn choices(&self) -> Vec<&Entry> {
        let query = self.query.text().trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.id.to_lowercase().contains(&query) || e.name.to_lowercase().contains(&query)
            })
            .collect()
    }
    pub fn highlighted(&self) -> Option<&str> {
        self.choices().get(self.selected).map(|e| e.id.as_str())
    }
    pub fn query(&self) -> &str {
        self.query.text()
    }
    pub fn paste(&mut self, text: &str) {
        self.query.insert_str(text);
        self.selected = 0;
    }
    pub fn navigate(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.choices().len().saturating_sub(1));
    }
    /// Route modal pointer input against the same native rows that are painted.
    /// Wheel movement only moves the highlight; a left click applies a theme.
    pub fn pointer_native(
        &mut self,
        style: &Style,
        bounds: R,
        action: &str,
        x: u16,
        y: u16,
        button: u8,
    ) -> Outcome {
        if !bounds.contains(x, y) {
            return Outcome::Consumed;
        }
        match action {
            "scroll_up" => self.navigate(-3),
            "scroll_down" => self.navigate(3),
            "down" if button == 0 => {
                if let Some(hit) = self
                    .native(style, bounds.width, bounds.height)
                    .hit(x - bounds.x, y - bounds.y)
                {
                    return self.action(&hit.action);
                }
            }
            _ => {}
        }
        Outcome::Consumed
    }
    /// Cell fallback uses identical filtering, row offsets and click semantics.
    pub fn pointer_cell(
        &mut self,
        area: Rect,
        x: u16,
        y: u16,
        down: bool,
        scroll: Option<bool>,
    ) -> Outcome {
        if !area.contains((x, y).into()) {
            return Outcome::Consumed;
        }
        if down && y == area.bottom().saturating_sub(1) && x >= area.right().saturating_sub(12) {
            return Outcome::Cancel;
        }
        if let Some(down) = scroll {
            self.navigate(if down { 3 } else { -3 });
        } else if down {
            let inner = crate::chrome::overlay::inner(area);
            let rows = usize::from(inner.height.saturating_sub(1));
            let first = self.selected.saturating_sub(rows.saturating_sub(1));
            if inner.contains((x, y).into()) && y > inner.y {
                if let Some(entry) = self.choices().get(first + usize::from(y - inner.y - 1)) {
                    return Outcome::Select(entry.id.clone());
                }
            }
        }
        Outcome::Consumed
    }
    /// Opaque actions from native rows. Validate against the displayed list so
    /// stale pointer targets cannot select a filtered-out theme.
    pub fn action(&self, action: &str) -> Outcome {
        if action == "theme-cancel" {
            return Outcome::Cancel;
        }
        if let Some(id) = action.strip_prefix("theme-select:") {
            if self.choices().iter().any(|e| e.id == id) {
                return Outcome::Select(id.into());
            }
            return Outcome::Consumed;
        }
        Outcome::Ignored
    }
    pub fn handle(&mut self, key: KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::Ignored;
        }
        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Enter => self
                .highlighted()
                .map(|id| Outcome::Select(id.into()))
                .unwrap_or(Outcome::Consumed),
            KeyCode::Up => {
                self.navigate(-1);
                Outcome::Consumed
            }
            KeyCode::Down => {
                self.navigate(1);
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.navigate(-8);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                self.navigate(8);
                Outcome::Consumed
            }
            _ => {
                let before = self.query.text().to_string();
                let result = self.query.handle(key);
                if before != self.query.text() {
                    self.selected = 0;
                }
                match result {
                    Edit::Ignored => Outcome::Ignored,
                    _ => Outcome::Consumed,
                }
            }
        }
    }
    /// Bridge KIT's named session keys without requiring a graphics feature.
    pub fn handle_named(&mut self, code: &str, modifiers: u8) -> Outcome {
        let key = if let Some(c) = code.strip_prefix("char:").and_then(|s| s.chars().next()) {
            KeyCode::Char(c)
        } else {
            match code {
                "escape" => KeyCode::Esc,
                "enter" => KeyCode::Enter,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "pageup" => KeyCode::PageUp,
                "pagedown" => KeyCode::PageDown,
                "backspace" => KeyCode::Backspace,
                "delete" => KeyCode::Delete,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                _ => return Outcome::Ignored,
            }
        };
        self.handle(KeyEvent::new(
            key,
            KeyModifiers::from_bits_truncate(modifiers),
        ))
    }
    /// A bounded physical-pixel dialog using KIT's default chrome. Mount after
    /// a Menu marker; the caller controls centering and modal input routing.
    pub fn native(&self, style: &Style, width: u16, height: u16) -> Surface {
        let mut s = style.modal_panel(width, height, "THEMES");
        if width < 128 || height < 124 {
            return s;
        }
        let query = format!("Search themes: {}▏", self.query.text());
        style.label(&mut s, R::new(16, 46, width - 32, 24), query);
        let rows = usize::from(height.saturating_sub(124) / 48);
        let first = self.selected.saturating_sub(rows.saturating_sub(1));
        for (row, (index, entry)) in self
            .choices()
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .enumerate()
        {
            let rect = R::new(12, 78 + row as u16 * 48, width - 24, 44);
            let swatches = if width >= 340 { 96 } else { 0 };
            style.row(
                &mut s,
                R::new(rect.x, rect.y, rect.width - swatches, rect.height),
                &entry.name,
                &entry.id,
                &format!("theme-select:{}", entry.id),
                index == self.selected,
                entry.id == self.current,
            );
            if swatches > 0 {
                let p = entry.palette;
                for (i, color) in [p.bg, p.fg, p.muted, p.accent].iter().enumerate() {
                    s.fill(
                        R::new(width - 102 + i as u16 * 21, rect.y + 12, 18, 18),
                        &crate::native_surface::style::hex(*color),
                        0,
                    );
                }
                s.hits.push(crate::native_surface::HitRegion {
                    rect: R::new(
                        rect.x + rect.width - swatches,
                        rect.y,
                        swatches,
                        rect.height,
                    ),
                    action: format!("theme-select:{}", entry.id),
                });
            }
        }
        if self.choices().is_empty() {
            style.caption(&mut s, R::new(16, 80, width - 32, 24), "No matching themes");
        }
        let footer = self
            .choices()
            .get(self.selected)
            .map(|e| e.source.as_str())
            .unwrap_or("Type to search");
        style.caption(
            &mut s,
            R::new(16, height - 42, width.saturating_sub(118), 18),
            footer,
        );
        style.caption(
            &mut s,
            R::new(16, height - 24, width.saturating_sub(118), 18),
            "↑ ↓ choose · Enter apply · Esc cancel",
        );
        style.tool(
            &mut s,
            R::new(width - 92, height - 42, 80, 28),
            "Cancel",
            "theme-cancel",
            false,
        );
        s
    }
    /// Cell fallback with the same filtering, highlight and selection model.
    pub fn render_cell(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        use crate::chrome::{overlay, rgb};
        let inner = overlay::render(
            area,
            buf,
            &overlay::Overlay {
                theme,
                title: "Themes",
                detail: None,
                footer: Some("↑ ↓ choose · Enter apply · Esc cancel"),
            },
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        buf.set_stringn(
            inner.x,
            inner.y,
            format!("Search: {}", self.query.text()),
            usize::from(inner.width),
            CellStyle::default().fg(rgb(theme.accent)),
        );
        let rows = usize::from(inner.height.saturating_sub(1));
        let first = self.selected.saturating_sub(rows.saturating_sub(1));
        for (row, (index, entry)) in self
            .choices()
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .enumerate()
        {
            let style = if index == self.selected {
                CellStyle::default()
                    .fg(rgb(theme.row_selected_fg))
                    .bg(rgb(theme.row_selected_bg))
            } else {
                CellStyle::default().fg(rgb(theme.fg))
            };
            buf.set_stringn(
                inner.x,
                inner.y + 1 + row as u16,
                format!(
                    "{} {}",
                    if entry.id == self.current { "✓" } else { " " },
                    entry.name
                ),
                usize::from(inner.width),
                style,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn picker() -> Picker {
        let registry = Registry::<Theme>::new(crate::paths::Paths::new(
            "starkit-picker-test",
            "STARKIT_PICKER_TEST_DIR",
            "STARKIT_PICKER_TEST_CONFIG_DIR",
        ));
        Picker::new(&registry, "kanagawa")
    }
    #[test]
    fn search_navigation_accept_and_cancel_share_one_state() {
        let mut p = picker();
        assert_eq!(p.highlighted(), Some("kanagawa"));
        assert_eq!(p.handle_named("escape", 0), Outcome::Cancel);
        p.paste("CATPPUCCIN");
        assert_eq!(p.highlighted(), Some("catppuccin-mocha"));
        p.navigate(1);
        assert_eq!(
            p.handle_named("enter", 0),
            Outcome::Select("catppuccin-latte".into())
        );
        assert_eq!(p.action("theme-select:kanagawa"), Outcome::Consumed);
        p.paste("missing-theme");
        p.navigate(999);
        assert_eq!(p.handle_named("enter", 0), Outcome::Consumed);
        assert_eq!(p.highlighted(), None);
        assert!(Picker::is_shortcut("char:t", 4));
        assert!(!Picker::is_shortcut("char:t", 0));
        assert!(!Picker::is_shortcut("char:t", 6));
    }
    #[test]
    fn pointer_navigation_uses_visible_rows_without_applying_on_scroll() {
        let mut p = picker();
        let bounds = R::new(30, 40, 680, 400);
        let original = p.highlighted().unwrap().to_string();
        assert_eq!(
            p.pointer_native(&Style::default(), bounds, "scroll_down", 0, 0, 0),
            Outcome::Consumed
        );
        assert_eq!(p.highlighted(), Some(original.as_str()));
        p.pointer_native(&Style::default(), bounds, "scroll_down", 100, 150, 0);
        assert_ne!(p.highlighted(), Some(original.as_str()));
        let s = p.native(&Style::default(), bounds.width, bounds.height);
        let h = s
            .hits
            .iter()
            .find(|h| h.action.starts_with("theme-select:"))
            .unwrap();
        let expected = p.action(&h.action);
        assert_eq!(
            p.pointer_native(
                &Style::default(),
                bounds,
                "down",
                bounds.x + h.rect.x + 1,
                bounds.y + h.rect.y + 1,
                1
            ),
            Outcome::Consumed
        );
        assert_eq!(
            p.pointer_native(
                &Style::default(),
                bounds,
                "down",
                bounds.x + h.rect.x + 1,
                bounds.y + h.rect.y + 1,
                0
            ),
            expected
        );
        let area = Rect::new(3, 5, 60, 12);
        p.pointer_cell(area, 8, 8, false, Some(true));
        let mut buffer = Buffer::empty(area);
        let registry = Registry::<Theme>::new(crate::paths::Paths::new(
            "starkit-picker-test",
            "STARKIT_PICKER_TEST_DIR",
            "STARKIT_PICKER_TEST_CONFIG_DIR",
        ));
        p.render_cell(area, &mut buffer, &registry.default_theme());
        let result = p.pointer_cell(area, area.x + 2, area.y + 2, true, None);
        assert!(matches!(result, Outcome::Select(_)));
    }
    #[test]
    fn native_and_cell_presentations_are_bounded() {
        let p = picker();
        for (w, h) in [
            (1, 1),
            (63, 100),
            (64, 120),
            (100, 144),
            (128, 124),
            (340, 120),
            (680, 540),
        ] {
            let s = p.native(&Style::default(), w, h);
            s.validate().unwrap();
            if h == 540 {
                assert!(s
                    .hits
                    .iter()
                    .any(|hit| hit.action == "theme-select:kanagawa"));
            }
        }
        let registry = Registry::<Theme>::new(crate::paths::Paths::new(
            "starkit-picker-test",
            "STARKIT_PICKER_TEST_DIR",
            "STARKIT_PICKER_TEST_CONFIG_DIR",
        ));
        for (w, h) in [(1, 1), (10, 3), (60, 20)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            p.render_cell(area, &mut buf, &registry.default_theme());
        }
    }
}
