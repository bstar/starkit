//! Searchable command guides shared by native and cell applications.
//! Applications supply their catalogue; browsing never executes a command.
use crate::{
    input::TextInput,
    native_surface::{style::Style, HitRegion, PixelRect as R, Surface},
    theme::Theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style as CellStyle},
};

/// A discoverable operation, including functions without a keyboard shortcut.
#[derive(Clone, Debug)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub category: String,
    pub shortcut: String,
    pub summary: String,
    pub instructions: String,
    pub keywords: String,
}
impl Command {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            category: category.into(),
            shortcut: String::new(),
            summary: String::new(),
            instructions: String::new(),
            keywords: String::new(),
        }
    }
    pub fn shortcut(mut self, text: impl Into<String>) -> Self {
        self.shortcut = text.into();
        self
    }
    pub fn summary(mut self, text: impl Into<String>) -> Self {
        self.summary = text.into();
        self
    }
    pub fn instructions(mut self, text: impl Into<String>) -> Self {
        self.instructions = text.into();
        self
    }
    pub fn keywords(mut self, text: impl Into<String>) -> Self {
        self.keywords = text.into();
        self
    }
}
struct Indexed {
    command: Command,
    title: String,
    keys: String,
    text: String,
    words: Vec<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Consumed,
    Close,
}

/// Bounded search input, category filters and selection shared by both views.
/// All input is modal. Unknown keys cannot reach the application underneath.
pub struct Guide {
    name: String,
    entries: Vec<Indexed>,
    categories: Vec<String>,
    query: TextInput,
    category: usize,
    selected: usize,
    detail_scroll: usize,
    detail_limit: std::cell::Cell<usize>,
    ranked: Vec<usize>,
    category_counts: Vec<usize>,
}
#[derive(Clone)]
struct Term {
    text: String,
    exclude: bool,
    phrase: bool,
    field: Field,
}
#[derive(Clone, Copy)]
enum Field {
    Any,
    Title,
    Shortcut,
    Category,
}
fn normalized(text: &str) -> String {
    let text = text
        .replace('↑', " up ")
        .replace('↓', " down ")
        .replace('←', " left ")
        .replace('→', " right ")
        .to_lowercase();
    text.chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|word| match word {
            "control" => "ctrl",
            "option" | "meta" => "alt",
            "esc" => "escape",
            "return" => "enter",
            "del" => "delete",
            "pgup" => "pageup",
            "pgdn" => "pagedown",
            _ => word,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn terms(query: &str) -> Vec<Term> {
    let mut result = Vec::new();
    let mut chars = query.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let exclude =
            chars.peek() == Some(&'-') && chars.clone().nth(1).is_some_and(|c| !c.is_whitespace());
        if exclude {
            chars.next();
        }
        let mut field = Field::Any;
        for (prefix, candidate) in [
            ("key:", Field::Shortcut),
            ("shortcut:", Field::Shortcut),
            ("category:", Field::Category),
            ("title:", Field::Title),
        ] {
            if chars
                .clone()
                .take(prefix.len())
                .collect::<String>()
                .eq_ignore_ascii_case(prefix)
            {
                for _ in 0..prefix.len() {
                    chars.next();
                }
                field = candidate;
                break;
            }
        }
        let phrase = chars.peek() == Some(&'"');
        if phrase {
            chars.next();
        }
        let mut value = String::new();
        for c in chars.by_ref() {
            if (phrase && c == '"') || (!phrase && c.is_whitespace()) {
                break;
            }
            value.push(c);
        }
        let text = normalized(&value);
        // Punctuation is meaningful in shortcuts, even when normalization is empty.
        let text = if text.is_empty() {
            value.trim().to_lowercase()
        } else {
            text
        };
        if !text.is_empty() {
            result.push(Term {
                text,
                exclude,
                phrase,
                field,
            });
        }
    }
    result
}
// One insertion, deletion, replacement or adjacent transposition. No unbounded DP.
fn typo(a: &str, b: &str) -> bool {
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    if a.len() < 4 || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let mut i = 0;
    while i < a.len().min(b.len()) && a[i] == b[i] {
        i += 1;
    }
    if i == a.len().min(b.len()) {
        return true;
    }
    if a.len() == b.len() {
        a[i + 1..] == b[i + 1..]
            || (i + 1 < a.len() && a[i] == b[i + 1] && a[i + 1] == b[i] && a[i + 2..] == b[i + 2..])
    } else if a.len() > b.len() {
        a[i + 1..] == b[i..]
    } else {
        a[i..] == b[i + 1..]
    }
}
impl Guide {
    pub fn new(name: impl Into<String>, commands: Vec<Command>) -> Self {
        let mut categories = vec!["All commands".to_string()];
        let entries = commands
            .into_iter()
            .map(|command| {
                if !categories.contains(&command.category) {
                    categories.push(command.category.clone());
                }
                let title = normalized(&command.title);
                let keys = normalized(&command.shortcut);
                let text = normalized(&format!(
                    "{} {} {} {} {} {}",
                    command.title,
                    command.category,
                    command.shortcut,
                    command.summary,
                    command.instructions,
                    command.keywords
                ));
                let words = text.split_whitespace().map(str::to_owned).collect();
                Indexed {
                    command,
                    title,
                    keys,
                    text,
                    words,
                }
            })
            .collect();
        let mut guide = Self {
            name: name.into(),
            entries,
            categories,
            query: TextInput::single().with_max_chars(256),
            category: 0,
            selected: 0,
            detail_scroll: 0,
            detail_limit: std::cell::Cell::new(0),
            ranked: Vec::new(),
            category_counts: Vec::new(),
        };
        guide.refresh_matches();
        guide
    }
    pub fn query(&self) -> &str {
        self.query.text()
    }
    pub fn search(&mut self, text: &str) {
        self.query.clear();
        self.paste(text);
    }
    pub fn paste(&mut self, text: &str) {
        self.query.insert_str(text);
        self.reset_selection();
    }
    fn reset_selection(&mut self) {
        self.selected = 0;
        self.detail_scroll = 0;
        self.refresh_matches();
    }
    fn score(entry: &Indexed, query: &[Term]) -> Option<usize> {
        let mut score = 0;
        for term in query {
            let scoped;
            let haystack = match term.field {
                Field::Any => &entry.text,
                Field::Title => &entry.title,
                Field::Shortcut => &entry.keys,
                Field::Category => {
                    scoped = normalized(&entry.command.category);
                    &scoped
                }
            };
            if !matches!(term.field, Field::Any) {
                // Field filters are precise; typo matching belongs to free text.
                let matched = haystack.contains(&term.text)
                    || (matches!(term.field, Field::Shortcut)
                        && entry.command.shortcut.to_lowercase().contains(&term.text));
                if matched == term.exclude {
                    return None;
                }
                continue;
            }
            let direct = entry.text.contains(&term.text)
                || entry.command.shortcut.to_lowercase().contains(&term.text);
            if term.exclude {
                if direct {
                    return None;
                }
                continue;
            }
            let weight = if entry.title == term.text || entry.keys == term.text {
                0
            } else if entry.title.starts_with(&term.text) {
                1
            } else if entry.title.contains(&term.text) || entry.keys.contains(&term.text) {
                2
            } else if direct {
                4
            } else if !term.phrase
                && entry
                    .title
                    .split_whitespace()
                    .any(|word| typo(&term.text, word))
            {
                8
            } else if !term.phrase && entry.words.iter().any(|word| typo(&term.text, word)) {
                12
            } else {
                return None;
            };
            score += weight;
        }
        Some(score)
    }
    fn refresh_matches(&mut self) {
        let query = terms(self.query.text());
        let mut scored: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Self::score(e, &query).map(|score| (score, i)))
            .collect();
        scored.sort_by_key(|&(score, i)| (score, i));
        self.ranked = scored.into_iter().map(|(_, i)| i).collect();
        self.category_counts = vec![0; self.categories.len()];
        self.category_counts[0] = self.ranked.len();
        for &i in &self.ranked {
            if let Some(category) = self
                .categories
                .iter()
                .position(|c| c == &self.entries[i].command.category)
            {
                self.category_counts[category] += 1;
            }
        }
    }
    fn matches_in(&self, category: usize) -> Vec<usize> {
        self.ranked
            .iter()
            .copied()
            .filter(|&i| {
                category == 0 || self.entries[i].command.category == self.categories[category]
            })
            .collect()
    }
    pub fn results(&self) -> Vec<&Command> {
        self.matches_in(self.category)
            .into_iter()
            .map(|i| &self.entries[i].command)
            .collect()
    }
    pub fn highlighted(&self) -> Option<&Command> {
        self.results().get(self.selected).copied()
    }
    pub fn navigate(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.results().len().saturating_sub(1));
        self.detail_scroll = 0;
    }
    pub fn cycle_category(&mut self, delta: isize) {
        self.category =
            (self.category as isize + delta).rem_euclid(self.categories.len() as isize) as usize;
        self.reset_selection();
    }
    pub fn handle(&mut self, key: KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::Consumed;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::F(1) => return Outcome::Close,
            KeyCode::Char('?') if self.query.is_empty() => return Outcome::Close,
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.cycle_category(-1),
            KeyCode::Tab => self.cycle_category(1),
            KeyCode::BackTab => self.cycle_category(-1),
            KeyCode::PageUp if ctrl => self.scroll_detail(-6),
            KeyCode::PageDown if ctrl => self.scroll_detail(6),
            KeyCode::Up => self.navigate(-1),
            KeyCode::Down => self.navigate(1),
            KeyCode::PageUp => self.navigate(-8),
            KeyCode::PageDown => self.navigate(8),
            KeyCode::Home if ctrl => {
                self.selected = 0;
                self.detail_scroll = 0;
            }
            KeyCode::End if ctrl => {
                self.selected = self.results().len().saturating_sub(1);
                self.detail_scroll = 0;
            }
            KeyCode::Char('f') if ctrl => {
                self.category = 0;
                self.query.set_cursor(self.query.text().len());
                self.reset_selection();
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.reset_selection();
            }
            _ => {
                let before = self.query.text().to_owned();
                self.query.handle(key);
                if before != self.query.text() {
                    self.reset_selection();
                }
            }
        }
        Outcome::Consumed
    }
    pub fn handle_named(&mut self, code: &str, modifiers: u8) -> Outcome {
        let code = if let Some(c) = code.strip_prefix("char:").and_then(|s| s.chars().next()) {
            KeyCode::Char(c)
        } else {
            match code {
                "escape" => KeyCode::Esc,
                "f1" | "f:1" => KeyCode::F(1),
                "tab" => KeyCode::Tab,
                "backtab" => KeyCode::BackTab,
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
                _ => return Outcome::Consumed,
            }
        };
        self.handle(KeyEvent::new(
            code,
            KeyModifiers::from_bits_truncate(modifiers),
        ))
    }
    pub fn action(&mut self, action: &str) -> Outcome {
        match action {
            "help-close" => return Outcome::Close,
            "help-clear" => {
                self.query.clear();
                self.reset_selection();
            }
            "help-all" => {
                self.category = 0;
                self.reset_selection();
            }
            "help-category-prev" => self.cycle_category(-1),
            "help-category-next" => self.cycle_category(1),
            "help-detail-up" => self.scroll_detail(-4),
            "help-detail-down" => self.scroll_detail(4),
            _ => {
                if let Some(i) = action
                    .strip_prefix("help-category:")
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&i| i < self.categories.len())
                {
                    self.category = i;
                    self.reset_selection();
                } else if let Some(i) = action
                    .strip_prefix("help-select:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    if let Some(index) = self
                        .matches_in(self.category)
                        .iter()
                        .position(|&id| id == i)
                    {
                        self.selected = index;
                        self.detail_scroll = 0;
                    }
                }
            }
        }
        Outcome::Consumed
    }
    fn scroll_detail(&mut self, delta: isize) {
        let limit = self.detail_limit.get();
        self.detail_scroll = self
            .detail_scroll
            .min(limit)
            .saturating_add_signed(delta)
            .min(limit);
    }
    fn details(&self) -> String {
        self.highlighted().map(|e| {
            let instructions=if e.instructions.is_empty(){&e.summary}else{&e.instructions};
            if e.shortcut.is_empty(){format!("{}\n\n{}",e.summary,instructions)}else{format!("Shortcut: {}\n\n{}",e.shortcut,instructions)}
        }).unwrap_or_else(|| "Try fewer words, another category, or Clear search. Search titles, shortcuts, gestures and instructions.".into())
    }
    /// Centered physical-pixel bounds shared by native rendering and pointer routing.
    pub fn native_rect(width: u16, height: u16) -> R {
        let w = width.saturating_sub(32).min(1140).max(width.min(240));
        let h = height.saturating_sub(32).min(760).max(height.min(220));
        R::new((width - w) / 2, (height - h) / 2, w, h)
    }
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
        let (x, y) = (x - bounds.x, y - bounds.y);
        if action == "scroll_up" || action == "scroll_down" {
            self.scroll_native(bounds.width, bounds.height, x, y, action == "scroll_down");
        } else if action == "down" && button == 0 {
            if let Some(hit) = self.native(style, bounds.width, bounds.height).hit(x, y) {
                return self.action(&hit.action);
            }
        }
        Outcome::Consumed
    }
    fn query_preview(&self, max: usize) -> String {
        let at = self.query.cursor();
        let before = &self.query.text()[..at];
        let start = before.chars().count().saturating_sub(max.saturating_sub(1));
        format!("{}▏{}", before, &self.query.text()[at..])
            .chars()
            .skip(start)
            .take(max)
            .collect()
    }
    fn native_geometry(width: u16, height: u16) -> (R, R, R) {
        let sidebar = if width >= 960 { 168 } else { 0 };
        let top = if height < 420 {
            if sidebar > 0 {
                82
            } else {
                112
            }
        } else if sidebar > 0 {
            120
        } else {
            150
        };
        let body = R::new(
            16,
            top,
            width.saturating_sub(32),
            height.saturating_sub(top + 54),
        );
        let list_x = body.x + sidebar;
        let remaining = body.width.saturating_sub(sidebar);
        let (list, detail) = if width >= 640 {
            let lw = remaining * 55 / 100;
            (
                R::new(list_x, body.y, lw.saturating_sub(12), body.height),
                R::new(
                    list_x + lw + 12,
                    body.y,
                    remaining.saturating_sub(lw + 12),
                    body.height,
                ),
            )
        } else {
            let dh = if body.height < 160 {
                0
            } else {
                (body.height / 3).min(176)
            };
            (
                R::new(
                    list_x,
                    body.y,
                    remaining,
                    body.height.saturating_sub(if dh > 0 { dh + 12 } else { 0 }),
                ),
                R::new(
                    list_x,
                    body.y + body.height.saturating_sub(dh),
                    remaining,
                    dh,
                ),
            )
        };
        (
            R::new(16, body.y, sidebar.saturating_sub(12), body.height),
            list,
            detail,
        )
    }
    pub fn native(&self, style: &Style, width: u16, height: u16) -> Surface {
        let mut s = style.modal_panel(width, height, &format!("{}  /  COMMAND GUIDE", self.name));
        if width < 240 || height < 220 {
            return s;
        }
        style.tool(
            &mut s,
            R::new(width - 78, 5, 62, 22),
            "Close",
            "help-close",
            false,
        );
        if height >= 420 {
            s.text(
                R::new(18, 42, width - 36, 22),
                "What would you like to do?",
                &style.colors.ink,
                18,
                true,
            );
        }
        let search_y = if height < 420 { 36 } else { 74 };
        let search = R::new(16, search_y, width - 32, 32);
        s.fill(search, &style.workspace.well, 3);
        style.focus(&mut s, search);
        let max = usize::from(width.saturating_sub(144) / 8);
        let visible = if self.query.is_empty() {
            "Search commands, shortcuts, gestures…"
                .chars()
                .take(max)
                .collect()
        } else {
            self.query_preview(max)
        };
        s.text(
            R::new(28, search_y + 4, width.saturating_sub(144), 24),
            visible.replace('▏', "|"),
            if self.query.is_empty() {
                &style.colors.dim
            } else {
                &style.colors.ink
            },
            14,
            false,
        );
        style.tool(
            &mut s,
            R::new(width - 100, search_y + 5, 72, 22),
            "Clear",
            "help-clear",
            false,
        );
        let (sidebar, list, detail) = Self::native_geometry(width, height);
        if sidebar.width == 0 {
            let category_y = search_y + 38;
            style.tool(
                &mut s,
                R::new(16, category_y, 58, 26),
                "All",
                "help-all",
                self.category == 0,
            );
            style.tool(
                &mut s,
                R::new(82, category_y, 30, 26),
                "‹",
                "help-category-prev",
                false,
            );
            style.tool(
                &mut s,
                R::new(118, category_y, width.saturating_sub(170), 26),
                &self.categories[self.category],
                "help-category-next",
                false,
            );
            style.tool(
                &mut s,
                R::new(width - 46, category_y, 30, 26),
                "›",
                "help-category-next",
                false,
            );
        }
        if sidebar.width > 0 {
            let rows = usize::from(sidebar.height / 30);
            let first = self.category.saturating_sub(rows.saturating_sub(1));
            for (row, i) in (first..self.categories.len()).take(rows).enumerate() {
                style.tool(
                    &mut s,
                    R::new(sidebar.x, sidebar.y + row as u16 * 30, sidebar.width, 26),
                    &format!("{}  {}", self.categories[i], self.category_counts[i]),
                    &format!("help-category:{i}"),
                    i == self.category,
                );
            }
        }
        let matches = self.matches_in(self.category);
        let rows = usize::from(list.height / 54);
        let first = self.selected.saturating_sub(rows.saturating_sub(1));
        for (row, (index, &id)) in matches
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .enumerate()
        {
            let e = &self.entries[id].command;
            let r = R::new(
                list.x,
                list.y + row as u16 * 54,
                list.width.saturating_sub(6),
                50,
            );
            let selected = index == self.selected;
            if selected {
                s.fill(
                    r,
                    &crate::native_surface::style::hex(style.palette.selected),
                    3,
                );
                s.fill(R::new(r.x, r.y + 5, 3, 40), &style.colors.accent, 1);
            }
            let key_w = if r.width >= 360 { 130 } else { 0 };
            s.text(
                R::new(r.x + 10, r.y + 4, r.width.saturating_sub(20 + key_w), 22),
                &e.title,
                &style.colors.ink,
                14,
                selected,
            );
            s.text(
                R::new(r.x + 10, r.y + 27, r.width.saturating_sub(20), 18),
                if key_w == 0 && !e.shortcut.is_empty() {
                    format!("{}   ·   {}", e.category, e.shortcut)
                } else {
                    e.category.clone()
                },
                &style.colors.dim,
                11,
                false,
            );
            if key_w > 0 && !e.shortcut.is_empty() {
                let key = R::new(r.x + r.width - key_w, r.y + 6, key_w - 8, 22);
                s.fill(key, &style.workspace.control, 3);
                s.text_centered(key, &e.shortcut, &style.colors.accent, 11, false);
            }
            s.hits.push(HitRegion {
                rect: r,
                action: format!("help-select:{id}"),
            });
        }
        if matches.len() > rows && rows > 0 {
            let track = list.height;
            let thumb = (usize::from(track) * rows / matches.len()).max(8) as u16;
            let y = list.y
                + ((usize::from(track - thumb) * first) / matches.len().saturating_sub(rows).max(1))
                    as u16;
            s.fill(
                R::new(list.x + list.width - 3, list.y, 2, track),
                &style.workspace.line,
                0,
            );
            s.fill(
                R::new(list.x + list.width - 3, y, 2, thumb),
                &style.colors.accent,
                0,
            );
        }
        if detail.height > 0 {
            s.fill(detail, &style.workspace.well, 3);
            let title = self
                .highlighted()
                .map(|e| e.title.as_str())
                .unwrap_or("No commands found");
            let title_rows = crate::wrap::wrap(title, detail.width.saturating_sub(24) / 9);
            let title_count = title_rows
                .len()
                .min(if detail.height < 180 { 1 } else { 2 });
            for (i, row) in title_rows.iter().take(title_count).enumerate() {
                s.text(
                    R::new(
                        detail.x + 12,
                        detail.y + 8 + i as u16 * 22,
                        detail.width.saturating_sub(24),
                        22,
                    ),
                    row.drawn(title),
                    &style.colors.ink,
                    16,
                    true,
                );
            }
            let text_top = 18 + title_count as u16 * 22;
            let text = self.details();
            let wrap = crate::wrap::wrap(&text, detail.width.saturating_sub(24) / 8);
            let count = usize::from(detail.height.saturating_sub(text_top + 32) / 20);
            self.detail_limit.set(wrap.len().saturating_sub(count));
            let offset = self.detail_scroll.min(self.detail_limit.get());
            for (line, row) in wrap.iter().skip(offset).take(count).enumerate() {
                s.text(
                    R::new(
                        detail.x + 12,
                        detail.y + text_top + line as u16 * 20,
                        detail.width.saturating_sub(24),
                        20,
                    ),
                    row.drawn(&text),
                    &style.colors.ink,
                    13,
                    false,
                );
            }
            if wrap.len() > count {
                style.tool(
                    &mut s,
                    R::new(detail.x + 12, detail.y + detail.height - 26, 30, 22),
                    "↑",
                    "help-detail-up",
                    false,
                );
                style.tool(
                    &mut s,
                    R::new(detail.x + 48, detail.y + detail.height - 26, 30, 22),
                    "↓",
                    "help-detail-down",
                    false,
                );
            }
        }
        let label = format!(
            "{} results  ·  {}",
            matches.len(),
            self.categories[self.category]
        );
        s.text(
            R::new(18, height - 48, width.saturating_sub(36), 18),
            label,
            &style.colors.accent,
            11,
            true,
        );
        s.text(
            R::new(18, height - 28, width.saturating_sub(36), 18),
            "Type to search  ·  ↑ ↓ browse  ·  Tab category  ·  Ctrl+U clear  ·  Esc close",
            &style.colors.dim,
            11,
            false,
        );
        s
    }
    /// Native scroll uses the same geometry as paint; details and results scroll independently.
    pub fn scroll_native(&mut self, width: u16, height: u16, x: u16, y: u16, down: bool) {
        let (_, _, detail) = Self::native_geometry(width, height);
        if detail.contains(x, y) {
            if down {
                self.scroll_detail(3);
            } else {
                self.scroll_detail(-3);
            }
        } else {
            self.navigate(if down { 3 } else { -3 });
        }
    }
    pub fn cell_rect(area: Rect) -> Rect {
        crate::chrome::overlay::rect(
            area,
            (40, 132),
            42,
            12,
            crate::chrome::overlay::Anchor::Centre,
        )
    }
    fn cell_geometry(area: Rect) -> (Rect, Rect, Rect) {
        let inner = crate::chrome::overlay::inner(Self::cell_rect(area));
        let body = Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(3),
        );
        if body.width >= 106 {
            let w = body.width * 55 / 100;
            (
                inner,
                Rect::new(body.x, body.y, w - 2, body.height),
                Rect::new(body.x + w, body.y, body.width - w, body.height),
            )
        } else {
            let h = (body.height / 2).min(7);
            (
                inner,
                Rect::new(
                    body.x,
                    body.y,
                    body.width,
                    body.height.saturating_sub(h + 1),
                ),
                Rect::new(
                    body.x,
                    body.y + body.height.saturating_sub(h),
                    body.width,
                    h,
                ),
            )
        }
    }
    pub fn render_cell(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        use crate::chrome::{overlay, rgb};
        let rect = Self::cell_rect(area);
        let (inner, list, detail) = Self::cell_geometry(area);
        let footer = "↑↓ browse · Tab category · Ctrl+U clear · Esc close";
        overlay::render(
            rect,
            buf,
            &overlay::Overlay {
                theme,
                title: &format!("{} / COMMAND GUIDE", self.name),
                detail: None,
                footer: Some(footer),
            },
        );
        if inner.width < 8 || inner.height < 5 {
            return;
        }
        let fg = CellStyle::default().fg(rgb(theme.fg));
        let dim = CellStyle::default().fg(rgb(theme.dim));
        let accent = CellStyle::default().fg(rgb(theme.accent));
        let query = if self.query.is_empty() {
            "Search commands, shortcuts, gestures…".into()
        } else {
            self.query_preview(usize::from(inner.width.saturating_sub(6)))
        };
        buf.set_stringn(
            inner.x,
            inner.y,
            format!("Find: {query}"),
            usize::from(inner.width),
            accent,
        );
        let matches = self.matches_in(self.category);
        buf.set_stringn(
            inner.x,
            inner.y + 1,
            format!(
                "‹ {} ›   {} results   · Tab changes category",
                self.categories[self.category],
                matches.len()
            ),
            usize::from(inner.width),
            dim,
        );
        let first = self
            .selected
            .saturating_sub(usize::from(list.height).saturating_sub(1));
        for (row, (index, &id)) in matches
            .iter()
            .enumerate()
            .skip(first)
            .take(usize::from(list.height))
            .enumerate()
        {
            let e = &self.entries[id].command;
            let style = if index == self.selected {
                CellStyle::default()
                    .fg(rgb(theme.row_selected_fg))
                    .bg(rgb(theme.row_selected_bg))
                    .add_modifier(Modifier::BOLD)
            } else {
                fg
            };
            let y = list.y + row as u16;
            buf.set_stringn(
                list.x,
                y,
                " ".repeat(usize::from(list.width)),
                usize::from(list.width),
                style,
            );
            let kw = (e.shortcut.chars().count() as u16 + 2).min(list.width / 2);
            buf.set_stringn(
                list.x,
                y,
                format!(
                    "{} {}",
                    if index == self.selected { "›" } else { " " },
                    e.title
                ),
                usize::from(list.width.saturating_sub(kw)),
                style,
            );
            buf.set_stringn(
                list.right().saturating_sub(kw),
                y,
                &e.shortcut,
                usize::from(kw),
                style,
            );
        }
        let title = self
            .highlighted()
            .map(|e| e.title.as_str())
            .unwrap_or("No commands found");
        if detail.height > 0 {
            buf.set_stringn(
                detail.x,
                detail.y,
                title,
                usize::from(detail.width),
                accent.add_modifier(Modifier::BOLD),
            );
        }
        let text = self.details();
        let lines = crate::wrap::wrap(&text, detail.width);
        let count = usize::from(detail.height.saturating_sub(1));
        self.detail_limit.set(lines.len().saturating_sub(count));
        let offset = self.detail_scroll.min(self.detail_limit.get());
        for (i, line) in lines.iter().skip(offset).take(count).enumerate() {
            buf.set_stringn(
                detail.x,
                detail.y + 1 + i as u16,
                line.drawn(&text),
                usize::from(detail.width),
                fg,
            );
        }
    }
    pub fn pointer_cell(
        &mut self,
        area: Rect,
        x: u16,
        y: u16,
        down: bool,
        scroll: Option<bool>,
    ) -> Outcome {
        let bounds = Self::cell_rect(area);
        if !bounds.contains((x, y).into()) {
            return Outcome::Consumed;
        }
        // The footer already labels this target "Esc close".
        if down && y == bounds.bottom().saturating_sub(1) && x >= bounds.right().saturating_sub(11)
        {
            return Outcome::Close;
        }
        let (inner, list, detail) = Self::cell_geometry(area);
        if let Some(down) = scroll {
            if detail.contains((x, y).into()) {
                if down {
                    self.scroll_detail(3);
                } else {
                    self.scroll_detail(-3);
                }
            } else {
                self.navigate(if down { 3 } else { -3 });
            }
        } else if down {
            if y == inner.y + 1 && x >= inner.x && x < inner.right() {
                self.cycle_category(1);
            }
            if list.contains((x, y).into()) {
                let matches = self.matches_in(self.category);
                let first = self
                    .selected
                    .saturating_sub(usize::from(list.height).saturating_sub(1));
                self.selected =
                    (first + usize::from(y - list.y)).min(matches.len().saturating_sub(1));
                self.detail_scroll = 0;
            }
        }
        Outcome::Consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn guide() -> Guide {
        Guide::new(
            "TEST",
            vec![
                Command::new("mic", "Select microphone", "Audio")
                    .shortcut("M")
                    .summary("Choose the narration input")
                    .keywords("voice sound device"),
                Command::new("all", "Capture all audio", "Audio")
                    .summary("Record the system output")
                    .keywords("speakers desktop sound"),
                Command::new("delete", "Delete clip", "Editing")
                    .shortcut("Delete")
                    .instructions("Select a clip; files stay on disk."),
                Command::new("theme", "Choose theme", "Appearance")
                    .shortcut("Alt+T")
                    .keywords("colour color palette"),
            ],
        )
    }
    #[test]
    fn search_ranks_and_combines_fields_without_requiring_word_order() {
        let mut g = guide();
        for (query, id) in [
            ("voice input", "mic"),
            ("input voice", "mic"),
            ("microhpone", "mic"),
            ("micrphone", "mic"),
            ("microphxne", "mic"),
            ("speakers", "all"),
            ("alt+t", "theme"),
            ("colour", "theme"),
            ("\"system output\"", "all"),
            ("sound -microphone", "all"),
        ] {
            g.search(query);
            assert_eq!(g.highlighted().map(|e| e.id.as_str()), Some(id), "{query}");
        }
        g.search("sound missing");
        assert!(g.results().is_empty());
        g.search("\"system input\"");
        assert!(g.results().is_empty());
        g.search("q");
        assert!(g.results().is_empty()); // no fuzzy single-letter floods
    }
    #[test]
    fn field_filters_combine_with_phrases_negation_and_general_search() {
        let mut g = guide();
        for (query, expected) in [
            ("category:audio voice", vec!["mic"]),
            ("category:audio -title:microphone", vec!["all"]),
            ("key:option+t", vec!["theme"]),
            ("key:delete", vec!["delete"]),
            ("title:\"capture all\"", vec!["all"]),
            ("-category:audio theme", vec!["theme"]),
            ("key:missing", vec![]),
        ] {
            g.search(query);
            assert_eq!(
                g.results()
                    .iter()
                    .map(|e| e.id.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{query}"
            );
        }
        g.search("CATEGORY:audio");
        assert_eq!(g.results().len(), 2);
    }
    #[test]
    fn a_typo_in_a_title_outranks_incidental_instruction_matches() {
        let mut g = Guide::new(
            "TEST",
            vec![
                Command::new("stop", "Stop recording", "Recording")
                    .instructions("Save microphone audio."),
                Command::new("mic", "Choose microphone", "Audio"),
            ],
        );
        g.search("microhpone");
        assert_eq!(g.highlighted().unwrap().id, "mic");
        g.search("option+t");
        assert!(g.results().is_empty());
        let mut g = guide();
        g.search("option+t");
        assert_eq!(g.highlighted().unwrap().id, "theme");
        g.search("é".repeat(100).as_str());
        g.handle_named("home", 0);
        assert!(g.query_preview(12).starts_with('▏'));
        assert_eq!(g.handle_named("f:1", 0), Outcome::Close);
    }
    #[test]
    fn input_is_modal_unicode_safe_and_category_counts_stay_consistent() {
        let mut g = guide();
        g.cycle_category(1);
        assert_eq!(g.results().len(), 2);
        g.search("not present");
        g.navigate(999);
        assert!(g.highlighted().is_none());
        g.handle_named("char:f", 2);
        g.handle_named("char:u", 2);
        assert_eq!(g.results().len(), 4);
        g.paste(&"🎙é日".repeat(200));
        assert_eq!(g.query().chars().count(), 256);
        g.handle_named("backspace", 0);
        g.handle_named("left", 0);
        g.handle_named("char:q", 0);
        assert_eq!(g.handle_named("escape", 0), Outcome::Close);
        g.search("clip");
        g.action("help-select:0");
        assert_eq!(g.highlighted().unwrap().id, "delete");
    }
    #[test]
    fn detail_scroll_stops_at_the_last_page_and_reverses_immediately() {
        let mut g = Guide::new(
            "TEST",
            vec![Command::new("long", "Long instructions", "Guide")
                .instructions("read this step carefully ".repeat(100))],
        );
        g.native(&Style::default(), 900, 500).validate().unwrap();
        let max = g.detail_limit.get();
        assert!(max > 0);
        for _ in 0..1000 {
            g.handle_named("pagedown", 2);
        }
        assert_eq!(g.detail_scroll, max);
        g.handle_named("pageup", 2);
        assert_eq!(g.detail_scroll, max.saturating_sub(6));
        let bounds = Guide::native_rect(1000, 700);
        assert_eq!(
            g.pointer_native(
                &Style::default(),
                bounds,
                "down",
                bounds.x + bounds.width - 47,
                bounds.y + 16,
                0
            ),
            Outcome::Close
        );
    }
    #[test]
    fn render_and_pointer_share_bounded_geometry() {
        let mut g = guide();
        let registry = crate::theme::Registry::<Theme>::new(crate::paths::Paths::new(
            "help-test",
            "HELP_TEST_DIR",
            "HELP_TEST_CONFIG_DIR",
        ));
        let (theme, _) = registry.resolve_named("kanagawa");
        for (w, h) in [(1, 1), (32, 16), (80, 24), (132, 42)] {
            let area = Rect::new(0, 0, w, h);
            let mut b = Buffer::empty(area);
            g.render_cell(area, &mut b, &theme);
            g.pointer_cell(area, 0, 0, false, Some(true));
        }
        for (w, h) in [(1, 1), (240, 220), (640, 480), (900, 700), (1100, 720)] {
            let s = g.native(&Style::default(), w, h);
            s.validate().unwrap();
        }
        // Short terminal windows must still expose a selectable result.
        for (w, h) in [(240, 220), (688, 328), (1100, 220)] {
            let surface = g.native(&Style::default(), w, h);
            surface.validate().unwrap();
            assert!(
                surface
                    .hits
                    .iter()
                    .any(|hit| hit.action.starts_with("help-select:")),
                "{w}x{h}"
            );
        }
    }
}
