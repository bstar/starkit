use super::{
    command::{Registry, Scope},
    layout, Frame, Id, Node, View, Viewport, Widget,
};
use crate::{
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind},
    input::{Edit, TextInput},
    theme::Theme,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Command(Id),
    /// A user moved keyboard focus with Tab or a pointer press.
    Focused(Id),
    Changed {
        id: Id,
        value: String,
    },
    Submitted {
        id: Id,
        value: String,
    },
    /// Escape in an inline editor. Modal Escape remains `CancelModal`.
    Cancelled {
        id: Id,
    },
    Selected {
        id: Id,
        item: Id,
    },
    /// Enter on an enabled selected list/table/tree item.
    Activated {
        id: Id,
        item: Id,
    },
    CancelModal,
    Adjusted {
        id: Id,
        value: u16,
        finished: bool,
    },
    CustomKey {
        id: Id,
        key: KeyEvent,
    },
    CustomPointer {
        id: Id,
        x: u16,
        y: u16,
        pressed: bool,
    },
    CustomScroll {
        id: Id,
        x: u16,
        y: u16,
        lines: i16,
    },
}

#[derive(PartialEq, Eq)]
struct ListPosition {
    selected: Option<Id>,
    index: Option<usize>,
    rows: usize,
}

/// Owns focus, editing, pointer capture, layout and invalidation. Drawing an
/// unchanged viewport returns the retained frame without solving layout again.
pub struct Runtime {
    pub(super) view: View,
    pub(super) registry: Registry,
    pub(super) theme: Theme,
    pub(super) visual: crate::native_surface::style::Style,
    pub(super) fields: BTreeMap<Id, TextInput>,
    field_modes: BTreeMap<Id, bool>,
    pub(super) scroll: BTreeMap<Id, usize>,
    list_positions: BTreeMap<Id, ListPosition>,
    frame: Option<Frame>,
    focus: Option<Id>,
    before_modal: Option<Id>,
    requested_focus: Option<Id>,
    capture: Option<(Id, u64)>,
    revision: u64,
    dirty: bool,
    layout_count: u64,
    layout_dirty: bool,
    skins: bool,
}
impl Runtime {
    pub fn new(view: View, registry: Registry, theme: Theme) -> anyhow::Result<Self> {
        layout::validate(&view)?;
        registry.validate_view(&view)?;
        let mut ui = Self {
            view,
            registry,
            visual: crate::native_surface::style::Style::new(
                crate::native_surface::style::Palette::for_chrome(&theme),
            ),
            theme,
            fields: BTreeMap::new(),
            field_modes: BTreeMap::new(),
            scroll: BTreeMap::new(),
            list_positions: BTreeMap::new(),
            frame: None,
            focus: None,
            before_modal: None,
            requested_focus: None,
            capture: None,
            revision: 0,
            dirty: true,
            layout_count: 0,
            layout_dirty: true,
            skins: true,
        };
        ui.sync_fields();
        Ok(ui)
    }
    pub fn commands(&self) -> &Registry {
        &self.registry
    }
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
    pub fn current_frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }
    pub fn needs_frame(&self) -> bool {
        self.dirty
    }
    pub fn layout_count(&self) -> u64 {
        self.layout_count
    }
    pub fn view(&self) -> &View {
        &self.view
    }
    pub fn set_view(&mut self, view: View) -> anyhow::Result<bool> {
        self.update(view, self.registry.clone())
    }
    pub fn set_commands(&mut self, registry: Registry) -> anyhow::Result<bool> {
        self.update(self.view.clone(), registry)
    }
    /// Install state and commands atomically, including newly mounted controls.
    pub fn update(&mut self, view: View, registry: Registry) -> anyhow::Result<bool> {
        layout::validate(&view)?;
        registry.validate_view(&view)?;
        if self.view == view && self.registry == registry {
            return Ok(false);
        }
        if self.view.modal.is_none() && view.modal.is_some() {
            self.before_modal = self.requested_focus.clone().or_else(|| self.focus.clone());
            self.focus = None;
            self.requested_focus = None;
        }
        if self.view.modal.is_some() && view.modal.is_none() {
            self.focus = self.before_modal.take();
            self.requested_focus = self.focus.clone();
        }
        let geometry_changed = !layout::same_geometry(&self.view, &view);
        if geometry_changed
            || !layout::same_targets(&self.view, &view)
            || !self.registry.same_targets(&registry)
        {
            self.capture = None;
        }
        self.layout_dirty |= geometry_changed;
        self.view = view;
        self.registry = registry;
        self.sync_fields();
        self.invalidate();
        Ok(true)
    }
    pub fn set_theme(&mut self, theme: Theme) {
        self.visual = crate::native_surface::style::Style::new(
            crate::native_surface::style::Palette::for_chrome(&theme),
        )
        .with_native_skins(self.skins);
        self.theme = theme;
        self.invalidate();
    }
    pub fn native_skins(&mut self, supported: bool) {
        self.skins = supported;
        self.visual = self.visual.clone().with_native_skins(supported);
        self.invalidate();
    }
    /// Repaint presentation after terminal loss without changing application state.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }
    pub fn captured(&self) -> Option<&Id> {
        self.capture.as_ref().map(|(id, _)| id)
    }
    pub fn cancel_pointer(&mut self) {
        self.capture = None;
    }
    /// Request a mounted control, including one temporarily hidden or disabled.
    /// The active modal still traps focus. A hidden request is remembered while
    /// the runtime chooses an available control, then restored when it returns.
    pub fn request_focus(&mut self, id: Id) -> anyhow::Result<bool> {
        let target = self
            .node_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Unknown focus target {}", id.0))?;
        anyhow::ensure!(
            target.accepts_focus(),
            "Control {} cannot receive focus",
            id.0
        );
        if self.requested_focus.as_ref() == Some(&id) {
            return Ok(false);
        }
        self.requested_focus = Some(id);
        self.cancel_pointer();
        self.invalidate();
        Ok(true)
    }
    pub fn frame(&mut self, viewport: Viewport) -> anyhow::Result<&Frame> {
        if self
            .frame
            .as_ref()
            .is_none_or(|frame| frame.viewport != viewport)
        {
            self.capture = None;
            self.dirty = true;
            self.layout_dirty = true;
        }
        if self.dirty {
            self.revision = self.revision.wrapping_add(1);
            let mut frame = if self.layout_dirty || self.frame.is_none() {
                self.layout_count += 1;
                layout::solve(&self.view, viewport, self.revision, self.focus.clone())?
            } else {
                let mut frame = self.frame.clone().expect("retained layout");
                layout::refresh(&self.view, &mut frame);
                frame.revision = self.revision;
                frame
            };
            for p in &mut frame.placements {
                if let Widget::Button { command } | Widget::Checkbox { command } = &p.widget {
                    p.enabled &= self.registry.get(command).is_some_and(|c| c.enabled);
                }
            }
            let order = frame.focus_order();
            if let Some(requested) = self
                .requested_focus
                .as_ref()
                .filter(|id| order.contains(id))
            {
                self.focus = Some(requested.clone());
            }
            if self.focus.as_ref().is_none_or(|id| !order.contains(id)) {
                self.focus = order.first().cloned();
            }
            frame.focus = self.focus.clone();
            for p in &frame.placements {
                if let Some(field) = self.fields.get_mut(&p.id) {
                    field.layout_width(match viewport.presentation {
                        super::Presentation::Cells => p.body.width,
                        super::Presentation::Graphics => 0,
                    });
                }
            }
            self.sync_list_scroll(&frame);
            self.frame = Some(frame);
            self.layout_dirty = false;
            self.dirty = false;
        }
        Ok(self.frame.as_ref().expect("frame installed"))
    }
    // Scroll belongs to input/layout state, not either renderer. A selected
    // item follows external reordering and viewport changes, while deliberate
    // wheel scrolling remains independent of selection.
    fn sync_list_scroll(&mut self, frame: &Frame) {
        for p in &frame.placements {
            if let Widget::List {
                items,
                selected,
                columns,
            } = &p.widget
            {
                let rows = usize::from(p.body.height / frame.viewport.scale(false))
                    .saturating_sub(usize::from(!columns.is_empty()))
                    .max(1);
                let index = selected
                    .as_ref()
                    .and_then(|id| items.iter().position(|item| &item.id == id));
                let position = ListPosition {
                    selected: selected.clone(),
                    index,
                    rows,
                };
                let changed = self.list_positions.get(&p.id) != Some(&position);
                let offset = self.scroll.entry(p.id.clone()).or_default();
                let previous = *offset;
                *offset = (*offset).min(items.len().saturating_sub(rows));
                if changed {
                    if let Some(index) = index {
                        if index < *offset {
                            *offset = index;
                        } else if index >= *offset + rows {
                            *offset = index + 1 - rows;
                        }
                    }
                }
                if previous != *offset && self.captured() == Some(&p.id) {
                    self.capture = None;
                }
                self.list_positions.insert(p.id.clone(), position);
            }
        }
    }
    fn prepare_input(&mut self) -> bool {
        let Some(viewport) = self.frame.as_ref().map(|frame| frame.viewport) else {
            return false;
        };
        self.frame(viewport).is_ok()
    }
    fn sync_fields(&mut self) {
        fn visit(node: &Node, result: &mut BTreeMap<Id, (String, bool)>) {
            if let Widget::Field {
                value, multiline, ..
            } = &node.widget
            {
                result.insert(node.id.clone(), (value.clone(), *multiline));
            }
            for child in &node.children {
                visit(child, result);
            }
        }
        fn lists(node: &Node, ids: &mut std::collections::BTreeSet<Id>) {
            if matches!(node.widget, Widget::List { .. }) {
                ids.insert(node.id.clone());
            }
            for child in &node.children {
                lists(child, ids);
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        lists(&self.view.root, &mut ids);
        if let Some(modal) = &self.view.modal {
            lists(modal, &mut ids);
        }
        self.scroll.retain(|id, _| ids.contains(id));
        self.list_positions.retain(|id, _| ids.contains(id));
        let mut values = BTreeMap::new();
        visit(&self.view.root, &mut values);
        if let Some(modal) = &self.view.modal {
            visit(modal, &mut values);
        }
        self.fields.retain(|id, _| values.contains_key(id));
        self.field_modes.retain(|id, _| values.contains_key(id));
        for (id, (value, multiline)) in values {
            if self.field_modes.insert(id.clone(), multiline) != Some(multiline) {
                self.fields.remove(&id);
            }
            let field = self.fields.entry(id).or_insert_with(|| {
                if multiline {
                    TextInput::multiline()
                } else {
                    TextInput::single()
                }
                .with_max_chars(65_536)
            });
            if field.text() != value {
                field.set_text(value);
            }
        }
    }
    fn node_mut(&mut self, id: &Id) -> Option<&mut Node> {
        fn find<'a>(node: &'a mut Node, id: &Id) -> Option<&'a mut Node> {
            if &node.id == id {
                return Some(node);
            }
            node.children.iter_mut().find_map(|child| find(child, id))
        }
        find(&mut self.view.root, id)
            .or_else(|| self.view.modal.as_mut().and_then(|node| find(node, id)))
    }
    fn edited(&mut self, id: Id) -> Event {
        let value = self.fields[&id].text().to_string();
        if let Some(Node {
            widget: Widget::Field { value: current, .. },
            ..
        }) = self.node_mut(&id)
        {
            *current = value.clone();
        }
        self.invalidate();
        Event::Changed { id, value }
    }
    pub fn paste(&mut self, text: &str) -> Vec<Event> {
        if !self.prepare_input() {
            return vec![];
        }
        let Some(id) = self.focus.clone() else {
            return vec![];
        };
        if let Some(field) = self.fields.get_mut(&id) {
            field.paste(text);
            vec![self.edited(id)]
        } else {
            vec![]
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Vec<Event> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        let Some(viewport) = self.frame.as_ref().map(|frame| frame.viewport) else {
            return vec![];
        };
        let Ok(frame) = self.frame(viewport).cloned() else {
            return vec![];
        };
        if key.code == KeyCode::Esc && self.view.modal.is_some() {
            self.cancel_pointer();
            return vec![Event::CancelModal];
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
            && !key
                .modifiers
                .contains(crate::crossterm::event::KeyModifiers::CONTROL)
        {
            let order = frame.focus_order();
            let index = self
                .focus
                .as_ref()
                .and_then(|id| order.iter().position(|candidate| candidate == id));
            if !order.is_empty() {
                let backwards = key.code == KeyCode::BackTab
                    || key
                        .modifiers
                        .contains(crate::crossterm::event::KeyModifiers::SHIFT);
                let next = match (index, backwards) {
                    (Some(i), false) => (i + 1) % order.len(),
                    (Some(i), true) => (i + order.len() - 1) % order.len(),
                    _ => 0,
                };
                let changed = self.focus.as_ref() != Some(&order[next]);
                self.focus = Some(order[next].clone());
                self.requested_focus = self.focus.clone();
                self.invalidate();
                if changed {
                    return vec![Event::Focused(order[next].clone())];
                }
            }
            return vec![];
        }
        if let Some(id) = self.focus.clone() {
            if let Some(p) = frame.get(&id) {
                if let Widget::Slider { value, max, step } = p.widget {
                    if key.modifiers.is_empty() {
                        let next = match key.code {
                            KeyCode::Left | KeyCode::Down => Some(value.saturating_sub(step)),
                            KeyCode::Right | KeyCode::Up => {
                                Some(value.saturating_add(step).min(max))
                            }
                            KeyCode::Home => Some(0),
                            KeyCode::End => Some(max),
                            _ => None,
                        };
                        if let Some(value) = next {
                            return self.adjust(id, value, true);
                        }
                    }
                }
            }
            if let Some(field) = self.fields.get_mut(&id) {
                let before = field.text().to_string();
                match field.handle(key) {
                    Edit::Consumed => {
                        if field.text() != before {
                            return vec![self.edited(id)];
                        }
                        self.invalidate();
                        return vec![];
                    }
                    Edit::Submit => {
                        return vec![Event::Submitted {
                            id,
                            value: field.text().into(),
                        }]
                    }
                    Edit::Cancel => return vec![Event::Cancelled { id }],
                    Edit::Ignored => {}
                }
            }
            if key.modifiers.is_empty() {
                if let Some(p) = frame.get(&id) {
                    if let Widget::List {
                        items,
                        selected,
                        columns,
                    } = &p.widget
                    {
                        if key.code == KeyCode::Enter {
                            if let Some(item) = selected.as_ref().and_then(|id| {
                                items.iter().find(|item| &item.id == id && item.enabled)
                            }) {
                                return vec![Event::Activated {
                                    id,
                                    item: item.id.clone(),
                                }];
                            }
                            return vec![];
                        }
                        let current = selected
                            .as_ref()
                            .and_then(|id| items.iter().position(|item| &item.id == id));
                        let page = usize::from(p.body.height / frame.viewport.scale(false))
                            .saturating_sub(usize::from(!columns.is_empty()))
                            .max(1);
                        let backwards =
                            matches!(key.code, KeyCode::Up | KeyCode::PageUp | KeyCode::End);
                        let target = match key.code {
                            KeyCode::Up => Some(
                                current
                                    .map_or(items.len().saturating_sub(1), |i| i.saturating_sub(1)),
                            ),
                            KeyCode::Down => Some(current.map_or(0, |i| i.saturating_add(1))),
                            KeyCode::PageUp => Some(current.map_or(0, |i| i.saturating_sub(page))),
                            KeyCode::PageDown => {
                                Some(current.map_or(0, |i| i.saturating_add(page)))
                            }
                            KeyCode::Home => Some(0),
                            KeyCode::End => Some(items.len().saturating_sub(1)),
                            _ => None,
                        };
                        if let Some(target) = target.filter(|_| !items.is_empty()) {
                            let target = target.min(items.len() - 1);
                            let item = if backwards {
                                items[..=target]
                                    .iter()
                                    .rev()
                                    .find(|item| item.enabled)
                                    .or_else(|| {
                                        items[target + 1..].iter().find(|item| item.enabled)
                                    })
                            } else {
                                items[target..]
                                    .iter()
                                    .find(|item| item.enabled)
                                    .or_else(|| {
                                        items[..target].iter().rev().find(|item| item.enabled)
                                    })
                            };
                            if let Some(item) = item {
                                return self.select(id, item.id.clone());
                            }
                            return vec![];
                        }
                    }
                }
            }
        }
        let ancestry = self
            .focus
            .as_ref()
            .map(|id| frame.ancestry(id))
            .unwrap_or_default();
        if let Some(command) = self.registry.resolve(key, &ancestry) {
            let allowed = self.view.modal.is_none()
                || matches!(command.scope, Scope::View(_))
                || matches!(
                    command.id.0.as_str(),
                    "kit.repaint" | "kit.help" | "kit.presentation"
                );
            if allowed {
                return vec![Event::Command(command.id.clone())];
            }
        }
        if key.modifiers.is_empty() && matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
            if let Some(id) = self.focus.clone() {
                return self.activate(&id);
            }
        }
        if let Some(id) = &self.focus {
            if frame
                .get(id)
                .is_some_and(|p| matches!(p.widget, Widget::Custom { .. }))
            {
                return vec![Event::CustomKey {
                    id: id.clone(),
                    key,
                }];
            }
        }
        vec![]
    }
    fn select(&mut self, id: Id, item: Id) -> Vec<Event> {
        if let Some(frame) = &self.frame {
            if let Some(p) = frame.get(&id) {
                if let Widget::List { items, columns, .. } = &p.widget {
                    if let Some(index) = items.iter().position(|row| row.id == item) {
                        let rows = usize::from(p.body.height / frame.viewport.scale(false))
                            .saturating_sub(usize::from(!columns.is_empty()))
                            .max(1);
                        let offset = self.scroll.entry(id.clone()).or_default();
                        let previous = *offset;
                        if index < *offset {
                            *offset = index;
                        } else if index >= *offset + rows {
                            *offset = index + 1 - rows;
                        }
                        if previous != *offset
                            && self.capture.as_ref().is_some_and(|(owner, _)| owner == &id)
                        {
                            self.capture = None;
                        }
                    }
                }
            }
        }
        if let Some(Node {
            widget: Widget::List { selected, .. },
            ..
        }) = self.node_mut(&id)
        {
            *selected = Some(item.clone());
        }
        self.invalidate();
        vec![Event::Selected { id, item }]
    }
    fn adjust(&mut self, id: Id, value: u16, finished: bool) -> Vec<Event> {
        if let Some(Node {
            widget: Widget::Slider { value: current, .. },
            ..
        }) = self.node_mut(&id)
        {
            *current = value;
        }
        self.invalidate();
        vec![Event::Adjusted {
            id,
            value,
            finished,
        }]
    }
    fn slider_pointer(&mut self, id: Id, x: u16, finished: bool) -> Vec<Event> {
        let Some(p) = self.frame.as_ref().and_then(|frame| frame.get(&id)) else {
            return vec![];
        };
        let Widget::Slider { max, .. } = p.widget else {
            return vec![];
        };
        let offset = x
            .saturating_sub(p.body.x)
            .min(p.body.width.saturating_sub(1));
        let value = (u32::from(offset) * u32::from(max)
            / u32::from(p.body.width.saturating_sub(1).max(1))) as u16;
        self.adjust(id, value, finished)
    }
    pub fn drag(&mut self, x: u16, y: u16) -> Vec<Event> {
        if !self.prepare_input() {
            return vec![];
        }
        let Some((id, _)) = &self.capture else {
            return vec![];
        };
        let Some(p) = self.frame.as_ref().and_then(|frame| frame.get(id)) else {
            return vec![];
        };
        if matches!(p.widget, Widget::Slider { .. }) {
            return self.slider_pointer(id.clone(), x, false);
        }
        if matches!(p.widget, Widget::Custom { .. }) {
            return vec![Event::CustomPointer {
                id: id.clone(),
                x: x.saturating_sub(p.body.x)
                    .min(p.body.width.saturating_sub(1)),
                y: y.saturating_sub(p.body.y)
                    .min(p.body.height.saturating_sub(1)),
                pressed: true,
            }];
        }
        vec![]
    }
    pub fn scroll(&mut self, x: u16, y: u16, lines: i16) -> Vec<Event> {
        if !self.prepare_input() {
            return vec![];
        }
        let Some(frame) = &self.frame else {
            return vec![];
        };
        let Some(p) = frame.hit(x, y) else {
            return vec![];
        };
        if matches!(p.widget, Widget::Custom { .. }) && lines != 0 {
            return vec![Event::CustomScroll {
                id: p.id.clone(),
                x: x - p.body.x,
                y: y - p.body.y,
                lines,
            }];
        }
        if let Widget::List { items, columns, .. } = &p.widget {
            let rows = usize::from(p.body.height / frame.viewport.scale(false))
                .saturating_sub(usize::from(!columns.is_empty()))
                .max(1);
            // A click started before scrolling must not select another row.
            self.capture = None;
            let offset = self.scroll.entry(p.id.clone()).or_default();
            *offset = offset
                .saturating_add_signed(isize::from(lines))
                .min(items.len().saturating_sub(rows));
            self.invalidate();
        }
        vec![]
    }
    fn activate(&mut self, id: &Id) -> Vec<Event> {
        let Some(p) = self.frame.as_ref().and_then(|f| f.get(id)) else {
            return vec![];
        };
        if !p.enabled {
            return vec![];
        }
        if let Widget::Button { command } | Widget::Checkbox { command } = &p.widget {
            if self.registry.get(command).is_some_and(|c| c.enabled) {
                return vec![Event::Command(command.clone())];
            }
        }
        vec![]
    }
    /// Press/release coordinates use the retained frame's units. A release must
    /// match its original control and viewport generation to invoke a command.
    pub fn pointer(&mut self, x: u16, y: u16, pressed: bool) -> Vec<Event> {
        if !self.prepare_input() {
            return vec![];
        }
        let Some(frame) = self.frame.clone() else {
            return vec![];
        };
        let hit = frame.hit(x, y).cloned();
        let mut events = vec![];
        if pressed {
            self.capture = hit
                .as_ref()
                .map(|p| (p.id.clone(), frame.viewport.generation));
            if let Some(p) = &hit {
                if self.focus.as_ref() != Some(&p.id) {
                    events.push(Event::Focused(p.id.clone()));
                }
                self.focus = Some(p.id.clone());
                self.requested_focus = self.focus.clone();
                self.invalidate();
                if matches!(p.widget, Widget::Slider { .. }) {
                    events.extend(self.slider_pointer(p.id.clone(), x, false));
                    return events;
                }
            }
        } else {
            let capture = self.capture.take();
            if let Some((id, generation)) = &capture {
                if *generation == frame.viewport.generation {
                    if let Some(p) = frame.get(id).filter(|p| p.enabled) {
                        if matches!(p.widget, Widget::Slider { .. }) {
                            return self.slider_pointer(id.clone(), x, true);
                        }
                        if matches!(p.widget, Widget::Custom { .. }) {
                            return vec![Event::CustomPointer {
                                id: id.clone(),
                                x: x.saturating_sub(p.body.x)
                                    .min(p.body.width.saturating_sub(1)),
                                y: y.saturating_sub(p.body.y)
                                    .min(p.body.height.saturating_sub(1)),
                                pressed: false,
                            }];
                        }
                    }
                }
            }
            if let Some(p) = &hit {
                if capture == Some((p.id.clone(), frame.viewport.generation)) {
                    if matches!(p.widget, Widget::Slider { .. }) {
                        return self.slider_pointer(p.id.clone(), x, true);
                    }
                    if let Widget::List { items, columns, .. } = &p.widget {
                        let row_height = frame.viewport.scale(false);
                        let row = usize::from(y.saturating_sub(p.body.y) / row_height);
                        if row < usize::from(!columns.is_empty()) {
                            return vec![];
                        }
                        let row = row.saturating_sub(usize::from(!columns.is_empty()))
                            + self.scroll.get(&p.id).copied().unwrap_or(0);
                        if let Some(item) = items.get(row).filter(|item| item.enabled) {
                            return self.select(p.id.clone(), item.id.clone());
                        }
                    }
                    if matches!(p.widget, Widget::Custom { .. }) {
                        return vec![Event::CustomPointer {
                            id: p.id.clone(),
                            x: x - p.body.x,
                            y: y - p.body.y,
                            pressed: false,
                        }];
                    }
                    return self.activate(&p.id);
                }
            }
        }
        if let Some(p) = hit.filter(|p| pressed && matches!(p.widget, Widget::Custom { .. })) {
            events.push(Event::CustomPointer {
                id: p.id,
                x: x - p.body.x,
                y: y - p.body.y,
                pressed,
            });
        }
        events
    }
}
