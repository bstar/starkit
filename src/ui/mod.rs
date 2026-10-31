//! Application presentation owned by KIT.
//!
//! Build a [`View`] from application state, install it in [`Runtime`], and route
//! input through the runtime. One immutable [`Frame`] supplies drawing and hit
//! geometry. Application commands never execute in a renderer or audio callback.
pub mod application;
pub mod command;
pub mod content;
mod layout;
mod render;
mod runtime;

pub use crate::panes::{Overflow, Size, Visibility, Wrap};
pub use layout::{ColumnPlacement, Frame, ListLayout, Placement, Presentation, Viewport};
pub use runtime::{Event, Runtime};

/// Stable within an application's UI, including across filtering and reordering.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub String);
impl From<&str> for Id {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for Id {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heading {
    pub title: String,
    pub status: String,
}
impl Heading {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            status: String::new(),
        }
    }
    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }
}

/// A list/table/tree row. Stable IDs preserve selection through data updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: Id,
    pub columns: Vec<String>,
    pub depth: u16,
    pub checked: bool,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A table column uses the same bounded logical sizes as application panes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub heading: String,
    pub width: Size,
    pub align: Align,
}
impl Column {
    pub fn new(heading: impl Into<String>) -> Self {
        Self {
            heading: heading.into(),
            width: Size::fill(1),
            align: Align::Left,
        }
    }
    pub fn width(mut self, width: Size) -> Self {
        self.width = width;
        self
    }
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}
impl Item {
    pub fn new(id: impl Into<Id>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            columns: vec![label.into()],
            depth: 0,
            checked: false,
            enabled: true,
        }
    }
}

/// All standard chrome and control semantics are interpreted by KIT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Widget {
    Row,
    Column,
    Pane(Heading),
    Label(String),
    Status(String),
    Button {
        command: Id,
    },
    Checkbox {
        command: Id,
    },
    Field {
        value: String,
        placeholder: String,
        secret: bool,
        multiline: bool,
    },
    List {
        items: std::sync::Arc<[Item]>,
        selected: Option<Id>,
        columns: Vec<String>,
    },
    /// A specialized view. The owner renders only inside `Placement::body`.
    /// The fallback is always shown when no cell implementation is installed.
    Custom {
        fallback: String,
        revision: u64,
    },
    Slider {
        value: u16,
        max: u16,
        step: u16,
    },
    Meter {
        value: u16,
        max: u16,
    },
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: Id,
    pub widget: Widget,
    pub children: Vec<Node>,
    pub width: Option<Size>,
    pub height: Option<Size>,
    pub visibility: Visibility,
    pub overflow: Overflow,
    pub priority: u16,
    pub wrap: Wrap,
    pub enabled: bool,
    /// Empty uses equal-width columns for a list with headings.
    pub table_columns: Vec<Column>,
}
impl Node {
    pub fn new(id: impl Into<Id>, widget: Widget) -> Self {
        Self {
            id: id.into(),
            widget,
            children: vec![],
            width: None,
            height: None,
            visibility: Visibility::Expanded,
            overflow: Overflow::Keep,
            priority: 0,
            wrap: Wrap::Never,
            enabled: true,
            table_columns: vec![],
        }
    }
    pub fn row(id: impl Into<Id>, children: Vec<Self>) -> Self {
        Self::new(id, Widget::Row).children(children)
    }
    pub fn column(id: impl Into<Id>, children: Vec<Self>) -> Self {
        Self::new(id, Widget::Column).children(children)
    }
    pub fn pane(id: impl Into<Id>, heading: Heading, child: Self) -> Self {
        Self::new(id, Widget::Pane(heading)).children(vec![child])
    }
    pub fn label(id: impl Into<Id>, text: impl Into<String>) -> Self {
        Self::new(id, Widget::Label(text.into()))
    }
    pub fn button(id: impl Into<Id>, command: impl Into<Id>) -> Self {
        Self::new(
            id,
            Widget::Button {
                command: command.into(),
            },
        )
    }
    /// A toolbar and tab strip are standard commands with shared interaction.
    pub fn toolbar(id: impl Into<Id>, commands: &[Id]) -> Self {
        let id = id.into();
        Self::row(
            id.clone(),
            commands
                .iter()
                .map(|command| Self::button(format!("{}/{}", id.0, command.0), command.clone()))
                .collect(),
        )
        .height(Size::fixed(1))
    }
    pub fn tabs(id: impl Into<Id>, commands: &[Id]) -> Self {
        Self::toolbar(id, commands)
    }
    /// Install this as `View::modal` for an exclusive keyboard/pointer menu.
    pub fn menu(id: impl Into<Id>, heading: Heading, commands: &[Id]) -> Self {
        let id = id.into();
        let items = commands
            .iter()
            .map(|command| Self::button(format!("{}/{}", id.0, command.0), command.clone()))
            .collect();
        Self::pane(
            id.clone(),
            heading,
            Self::column(format!("{}/items", id.0), items),
        )
    }
    pub fn list(id: impl Into<Id>, items: Vec<Item>, selected: Option<Id>) -> Self {
        Self::new(
            id,
            Widget::List {
                items: items.into(),
                selected,
                columns: vec![],
            },
        )
    }
    pub fn table(
        id: impl Into<Id>,
        columns: Vec<String>,
        items: Vec<Item>,
        selected: Option<Id>,
    ) -> Self {
        Self::new(
            id,
            Widget::List {
                items: items.into(),
                selected,
                columns,
            },
        )
    }
    pub fn slider(id: impl Into<Id>, value: u16, max: u16) -> Self {
        Self::new(
            id,
            Widget::Slider {
                value: value.min(max.max(1)),
                max: max.max(1),
                step: 1,
            },
        )
    }
    pub fn table_with_columns(
        id: impl Into<Id>,
        columns: Vec<Column>,
        items: Vec<Item>,
        selected: Option<Id>,
    ) -> Self {
        let mut node = Self::table(
            id,
            columns.iter().map(|c| c.heading.clone()).collect(),
            items,
            selected,
        );
        node.table_columns = columns;
        node
    }
    pub fn meter(id: impl Into<Id>, value: u16, max: u16) -> Self {
        Self::new(
            id,
            Widget::Meter {
                value: value.min(max.max(1)),
                max: max.max(1),
            },
        )
    }
    pub fn field(id: impl Into<Id>, value: impl Into<String>) -> Self {
        Self::new(
            id,
            Widget::Field {
                value: value.into(),
                placeholder: String::new(),
                secret: false,
                multiline: false,
            },
        )
    }
    /// A message composer: Enter submits, Shift/Alt+Enter inserts a newline.
    pub fn multiline(id: impl Into<Id>, value: impl Into<String>) -> Self {
        let mut node = Self::field(id, value);
        if let Widget::Field { multiline, .. } = &mut node.widget {
            *multiline = true;
        }
        node
    }
    /// Mask the presentation in both cells and native surfaces. The value in
    /// application state and edit events remains the original text.
    pub fn password(id: impl Into<Id>, value: impl Into<String>) -> Self {
        let mut node = Self::field(id, value);
        if let Widget::Field { secret, .. } = &mut node.widget {
            *secret = true;
        }
        node
    }
    pub fn children(mut self, children: Vec<Self>) -> Self {
        self.children = children;
        self
    }
    /// Sizes use logical UI units: one column/row in cells, 8/24 physical pixels.
    pub fn width(mut self, size: Size) -> Self {
        self.width = Some(size);
        self
    }
    pub fn height(mut self, size: Size) -> Self {
        self.height = Some(size);
        self
    }
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.visibility = if hidden {
            Visibility::Hidden
        } else {
            Visibility::Expanded
        };
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }
    pub fn priority(mut self, priority: u16) -> Self {
        self.priority = priority;
        self
    }
    pub fn wrap(mut self, wrap: Wrap) -> Self {
        self.wrap = wrap;
        self
    }
    pub(crate) fn focusable(&self) -> bool {
        self.enabled && self.accepts_focus()
    }
    pub(crate) fn accepts_focus(&self) -> bool {
        matches!(
            self.widget,
            Widget::Button { .. }
                | Widget::Checkbox { .. }
                | Widget::Field { .. }
                | Widget::List { .. }
                | Widget::Custom { .. }
                | Widget::Slider { .. }
        )
    }
}

/// Modal content uses the same tree and controls, with exclusive focus/input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub root: Node,
    pub modal: Option<Node>,
}
impl View {
    pub fn new(root: Node) -> Self {
        Self { root, modal: None }
    }
    pub fn modal(mut self, modal: Node) -> Self {
        self.modal = Some(modal);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        theme::{Theme, ThemeFile, BUILTINS},
    };
    use command::{Command, Registry, Scope};
    fn theme() -> Theme {
        Theme::resolve(&ThemeFile::parse(BUILTINS[0].toml).unwrap())
    }
    fn registry() -> Registry {
        Registry::new(vec![
            Command::new("open", "Open").shortcut("ctrl+o"),
            Command::new("off", "Unavailable").enabled(false),
        ])
        .unwrap()
    }
    fn view() -> View {
        View::new(Node::pane(
            "main",
            Heading::new("Workspace"),
            Node::column(
                "content",
                vec![
                    Node::field("name", ""),
                    Node::button("open-button", "open"),
                    Node::button("off-button", "off"),
                ],
            ),
        ))
    }
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    fn chrome_padding_aligns_text_and_preserves_explicit_dialog_size() {
        let base = View::new(Node::pane(
            "panel",
            Heading::new("Aligned heading"),
            Node::column(
                "content",
                vec![
                    Node::label("label", "Field label"),
                    Node::field("field", "Field value"),
                    Node::table(
                        "list",
                        vec!["Column heading".into()],
                        vec![Item::new("item", "List value")],
                        None,
                    ),
                ],
            ),
        ));
        let mut ui = Runtime::new(base.clone(), Registry::default(), theme()).unwrap();
        let viewport = Viewport::graphics(800, 600);
        let surface = ui.render_surface(viewport).unwrap();
        let left_edges: Vec<_> = surface
            .nodes
            .iter()
            .filter_map(|node| match node {
                crate::native_surface::Primitive::Text { rect, text, .. }
                    if [
                        "Aligned heading",
                        "Field label",
                        "Field value",
                        "Column heading",
                        "List value",
                    ]
                    .contains(&text.as_str()) =>
                {
                    Some(rect.x)
                }
                _ => None,
            })
            .collect();
        assert_eq!(left_edges.len(), 5);
        assert!(left_edges.iter().all(|x| *x == left_edges[0]));
        let panel = ui.current_frame().unwrap().get(&"panel".into()).unwrap();
        assert_eq!(
            panel.area,
            crate::ratatui::layout::Rect::new(8, 8, 784, 584)
        );
        ui.set_view(
            base.modal(
                Node::pane(
                    "dialog",
                    Heading::new("Dialog"),
                    Node::field("modal-field", ""),
                )
                .width(Size::fixed(40))
                .height(Size::fixed(12)),
            ),
        )
        .unwrap();
        let frame = ui.frame(viewport).unwrap();
        let dialog = frame.get(&"dialog".into()).unwrap();
        assert_eq!((dialog.area.width, dialog.area.height), (320, 288));
        assert_eq!((dialog.area.x, dialog.area.y), (240, 156));
        let frame = ui.frame(Viewport::cells(100, 40)).unwrap();
        let dialog = frame.get(&"dialog".into()).unwrap();
        assert_eq!((dialog.area.width, dialog.area.height), (40, 12));
        assert_eq!(dialog.body.x, dialog.area.x + 1);
        ui.render_surface(Viewport::graphics(12, 12))
            .unwrap()
            .validate()
            .unwrap();
    }
    #[test]
    fn quiet_frames_do_not_relayout_and_equal_views_do_not_invalidate() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let v = Viewport::cells(80, 24);
        let first = ui.frame(v).unwrap().clone();
        for _ in 0..100 {
            assert_eq!(&first, ui.frame(v).unwrap());
        }
        assert!(!ui.set_view(view()).unwrap());
        assert!(!ui.needs_frame());
        assert_eq!(ui.layout_count(), 1);
    }
    #[test]
    fn text_editing_does_not_trigger_commands_and_keeps_unicode() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        ui.frame(Viewport::cells(80, 24)).unwrap();
        assert_eq!(
            ui.key(key(KeyCode::Char('日'))),
            vec![Event::Changed {
                id: "name".into(),
                value: "日".into()
            }]
        );
        assert_eq!(
            ui.paste("本語"),
            vec![Event::Changed {
                id: "name".into(),
                value: "日本語".into()
            }]
        );
        assert_eq!(
            ui.key(key(KeyCode::Enter)),
            vec![Event::Submitted {
                id: "name".into(),
                value: "日本語".into()
            }]
        );
    }
    #[test]
    fn disabled_controls_are_skipped_and_keyboard_matches_pointer() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let v = Viewport::cells(80, 24);
        ui.frame(v).unwrap();
        ui.key(key(KeyCode::Tab));
        let frame = ui.frame(v).unwrap().clone();
        assert_eq!(frame.focus, Some("open-button".into()));
        let expected = vec![Event::Command("open".into())];
        assert_eq!(ui.key(key(KeyCode::Enter)), expected);
        let area = frame.get(&"open-button".into()).unwrap().area;
        assert!(ui.pointer(area.x, area.y, true).is_empty());
        ui.frame(v).unwrap();
        assert_eq!(ui.pointer(area.x, area.y, false), expected);
        ui.key(key(KeyCode::Tab));
        assert_eq!(ui.frame(v).unwrap().focus, Some("name".into()));
    }
    #[test]
    fn resize_cancels_pressed_control() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let v = Viewport::cells(80, 24);
        let area = ui
            .frame(v)
            .unwrap()
            .get(&"open-button".into())
            .unwrap()
            .area;
        ui.pointer(area.x, area.y, true);
        ui.frame(Viewport { generation: 2, ..v }).unwrap();
        assert!(ui.pointer(area.x, area.y, false).is_empty());
    }
    #[test]
    fn modal_traps_focus_and_commands_and_restores_focus() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let v = Viewport::cells(80, 24);
        ui.frame(v).unwrap();
        ui.set_view(view().modal(Node::pane(
            "dialog",
            Heading::new("Dialog"),
            Node::field("dialog-field", ""),
        )))
        .unwrap();
        assert_eq!(ui.frame(v).unwrap().focus, Some("dialog-field".into()));
        assert!(ui
            .key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
            .is_empty());
        ui.key(key(KeyCode::Tab));
        assert_eq!(ui.frame(v).unwrap().focus, Some("dialog-field".into()));
        assert_eq!(ui.key(key(KeyCode::Esc)), vec![Event::CancelModal]);
        ui.set_view(view()).unwrap();
        assert_eq!(ui.frame(v).unwrap().focus, Some("name".into()));
    }
    #[test]
    fn registry_rejects_conflicts_and_allows_disjoint_view_scopes() {
        assert!(Registry::new(vec![
            Command::new("a", "A").shortcut("ctrl+s"),
            Command::new("b", "B").shortcut("ctrl+s")
        ])
        .is_err());
        assert!(Registry::new(vec![Command::new("a", "A")
            .shortcut("ctrl+s")
            .shortcut("ctrl+s")])
        .is_err());
        assert!(Registry::new(vec![Command::new("a", "A").shortcut("gg")]).is_err());
        let commands = Registry::new(vec![
            Command::new("a", "A")
                .shortcut("ctrl+s")
                .scope(Scope::View("left".into())),
            Command::new("b", "B")
                .shortcut("ctrl+s")
                .scope(Scope::View("right".into())),
        ])
        .unwrap();
        assert_eq!(
            commands
                .resolve(
                    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                    &["right".into()]
                )
                .unwrap()
                .id,
            "b".into()
        );
    }
    #[test]
    fn duplicate_tree_ids_are_rejected_atomically() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let invalid = View::new(Node::column(
            "root",
            vec![Node::label("same", "one"), Node::label("same", "two")],
        ));
        assert!(ui.set_view(invalid).is_err());
        assert_eq!(ui.view(), &view());
    }
    #[test]
    fn every_theme_and_tiny_viewport_has_bounded_rendering() {
        for builtin in BUILTINS {
            let t = Theme::resolve(&ThemeFile::parse(builtin.toml).unwrap());
            let mut ui = Runtime::new(view(), registry(), t).unwrap();
            for (w, h) in [(0, 0), (1, 1), (8, 4), (80, 24)] {
                ui.render_cells(Viewport::cells(w, h)).unwrap();
            }
            for (w, h) in [(1, 1), (20, 10), (800, 480)] {
                ui.render_surface(Viewport::graphics(w, h))
                    .unwrap()
                    .validate()
                    .unwrap();
            }
        }
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn both_presentations_use_the_same_controls_and_actions() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let v = crate::terminal_graphics::Viewport::default();
        let cells = ui.render_scene(v, true).unwrap();
        assert!(!cells.spans.is_empty());
        let cell_order = ui.current_frame().unwrap().focus_order();
        let native = ui.render_scene(v, false).unwrap();
        assert_eq!(cell_order, ui.current_frame().unwrap().focus_order());
        let pixels = crate::terminal_graphics::renderer::render_reference(&native).unwrap();
        assert_eq!(pixels.dimensions(), (v.width, v.height));
    }
    #[test]
    fn edits_and_focus_reuse_geometry_but_update_rendered_values() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let v = Viewport::cells(80, 24);
        ui.frame(v).unwrap();
        ui.key(key(KeyCode::Char('a')));
        let buffer = ui.render_cells(v).unwrap();
        let rect = ui
            .current_frame()
            .unwrap()
            .get(&"name".into())
            .unwrap()
            .area;
        assert_eq!(buffer[(rect.x, rect.y)].symbol(), "a");
        ui.key(key(KeyCode::Tab));
        ui.frame(v).unwrap();
        assert_eq!(ui.layout_count(), 1);
    }
    #[test]
    fn custom_cell_content_is_local_and_cannot_overwrite_chrome() {
        struct Fill;
        impl content::Content for Fill {
            fn cells(&mut self, _: &Id, buffer: &mut crate::ratatui::buffer::Buffer) -> bool {
                for cell in &mut buffer.content {
                    cell.set_symbol("x");
                }
                true
            }
        }
        let view = View::new(Node::pane(
            "panel",
            Heading::new("Title"),
            Node::new(
                "custom",
                Widget::Custom {
                    fallback: "unavailable".into(),
                    revision: 0,
                },
            ),
        ));
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        let buffer = ui
            .render_cells_with(Viewport::cells(20, 8), &mut Fill)
            .unwrap();
        assert_ne!(buffer[(0, 0)].symbol(), "x");
        assert_eq!(buffer[(1, 1)].symbol(), "x");
        assert_ne!(buffer[(19, 7)].symbol(), "x");
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn canvas_layers_share_their_content_transform_and_stay_below_modal() {
        struct Canvas;
        impl content::Content for Canvas {
            fn canvas(&mut self, _: &Id) -> Option<content::Canvas> {
                Some(content::Canvas {
                    stream: "bounded-stream".into(),
                    source: [1920, 1080],
                    interactive: true,
                })
            }
        }
        let view = View::new(Node::pane(
            "panel",
            Heading::new("Canvas"),
            Node::new(
                "canvas",
                Widget::Custom {
                    fallback: "unavailable".into(),
                    revision: 0,
                },
            ),
        ))
        .modal(Node::pane(
            "modal",
            Heading::new("Dialog"),
            Node::label("notice", "Above video"),
        ));
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        let scene = ui
            .render_scene_with(
                crate::terminal_graphics::Viewport::default(),
                false,
                &mut Canvas,
            )
            .unwrap();
        let canvas = scene
            .components
            .iter()
            .position(|c| matches!(c, crate::terminal_graphics::Component::LiveCanvas { .. }))
            .unwrap();
        let dialog = scene
            .components
            .iter()
            .position(|c| matches!(c, crate::terminal_graphics::Component::Dialog { .. }))
            .unwrap();
        assert!(canvas < dialog);
        crate::terminal_graphics::renderer::render_reference(&scene).unwrap();
    }
    #[test]
    fn slider_keyboard_and_drag_share_value_and_finish_semantics() {
        let view = View::new(Node::slider("volume", 25, 100));
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        let viewport = Viewport::cells(21, 3);
        ui.frame(viewport).unwrap();
        assert_eq!(
            ui.key(key(KeyCode::Right)),
            vec![Event::Adjusted {
                id: "volume".into(),
                value: 26,
                finished: true
            }]
        );
        ui.frame(viewport).unwrap();
        assert_eq!(
            ui.pointer(10, 0, true),
            vec![Event::Adjusted {
                id: "volume".into(),
                value: 50,
                finished: false
            }]
        );
        assert_eq!(
            ui.drag(20, 0),
            vec![Event::Adjusted {
                id: "volume".into(),
                value: 100,
                finished: false
            }]
        );
        assert_eq!(
            ui.pointer(20, 0, false),
            vec![Event::Adjusted {
                id: "volume".into(),
                value: 100,
                finished: true
            }]
        );
    }
    #[test]
    fn mounted_nested_shortcut_scopes_are_rejected() {
        let commands = Registry::new(vec![
            Command::new("a", "A")
                .shortcut("ctrl+s")
                .scope(Scope::View("outer".into())),
            Command::new("b", "B")
                .shortcut("ctrl+s")
                .scope(Scope::View("inner".into())),
        ])
        .unwrap();
        let view = View::new(Node::column("outer", vec![Node::label("inner", "Nested")]));
        assert!(Runtime::new(view, commands, theme()).is_err());
    }
    #[test]
    fn custom_capture_survives_visual_updates_and_releases_to_its_owner() {
        let view = |revision| {
            View::new(Node::row(
                "row",
                vec![
                    Node::new(
                        "a",
                        Widget::Custom {
                            fallback: "a".into(),
                            revision,
                        },
                    ),
                    Node::new(
                        "b",
                        Widget::Custom {
                            fallback: "b".into(),
                            revision: 0,
                        },
                    ),
                ],
            ))
        };
        let mut ui = Runtime::new(view(0), Registry::default(), theme()).unwrap();
        let viewport = Viewport::cells(40, 10);
        ui.frame(viewport).unwrap();
        ui.pointer(1, 1, true);
        ui.set_view(view(1)).unwrap();
        assert_eq!(ui.captured(), Some(&"a".into()));
        ui.frame(viewport).unwrap();
        let events = ui.pointer(39, 1, false);
        assert!(
            matches!(&events[..],[Event::CustomPointer { id, pressed:false, .. }] if id.0 == "a")
        );
        assert!(ui.pointer(39, 1, false).is_empty());
    }
    #[test]
    fn automatic_hiding_restores_requested_focus() {
        let mut ui = Runtime::new(view(), registry(), theme()).unwrap();
        let viewport = Viewport::cells(80, 24);
        ui.frame(viewport).unwrap();
        ui.key(key(KeyCode::Tab));
        ui.frame(viewport).unwrap();
        let mut hidden = view();
        hidden.root.children[0].children[1].visibility = Visibility::Hidden;
        ui.set_view(hidden).unwrap();
        assert_eq!(ui.frame(viewport).unwrap().focus, Some("name".into()));
        ui.set_view(view()).unwrap();
        assert_eq!(
            ui.frame(viewport).unwrap().focus,
            Some("open-button".into())
        );
    }
    #[test]
    fn secret_and_long_native_fields_have_bounded_text_and_utf8_carets() {
        let secret = "秘密password".repeat(1000);
        let view = View::new(Node::new(
            "secret",
            Widget::Field {
                value: secret.clone(),
                placeholder: String::new(),
                secret: true,
                multiline: false,
            },
        ));
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        let surface = ui.render_surface(Viewport::graphics(640, 32)).unwrap();
        assert_eq!(surface.text_carets.len(), 1);
        surface.validate().unwrap();
        let serialized = serde_json::to_string(&surface).unwrap();
        assert!(!serialized.contains("password"));
        assert!(!serialized.contains("秘密"));
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn scrolling_changes_click_admission_but_keeps_wheel_destinations_stable() {
        let items = (0..100)
            .map(|i| Item::new(format!("item-{i}"), format!("Item {i}")))
            .collect();
        let view = View::new(Node::list("list", items, None));
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        let viewport = crate::terminal_graphics::Viewport::default();
        let before = ui.render_scene(viewport, false).unwrap();
        ui.scroll(10, 10, 3);
        let after = ui.render_scene(viewport, false).unwrap();
        assert_ne!(before.interaction, after.interaction);
        assert_eq!(before.scroll_interaction, after.scroll_interaction);
    }
}

#[cfg(test)]
mod control_tests;
