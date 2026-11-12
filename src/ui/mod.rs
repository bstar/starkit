//! Application presentation owned by KIT.
//!
//! Build a [`View`] from application state, install it in [`Runtime`], and route
//! input through the runtime. One immutable [`Frame`] supplies drawing and hit
//! geometry. Application commands never execute in a renderer or audio callback.
pub mod application;
pub mod command;
pub mod content;
mod layout;
mod reload;
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
    /// Optional logical spacing; defaults preserve existing layouts.
    pub gap: Option<u16>,
    /// Container inset, or horizontal field inset, in graphical pixels; rounded to cells in the fallback.
    pub padding_pixels: u16,
    /// Exact graphical group spacing, taking precedence over logical gap.
    pub gap_pixels: Option<u16>,
    pub font_size: Option<u16>,
    /// Semantic primary action, rendered with the shared accent.
    pub primary: bool,
    /// Optional theme-derived color for this primary action only.
    pub primary_color: Option<crate::theme::color::Rgb>,
    pub navigation: bool,
    /// Framework vector icon beside a button label; independent of font glyphs.
    pub icon: Option<String>,
    pub icon_size: u16,
    /// Subtle alternating rows for dense data lists.
    pub striped: bool,
    /// Modal menu origin in viewport units; KIT clamps it to the visible area.
    pub popup_position: Option<(u16, u16)>,
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
            gap: None,
            padding_pixels: 0,
            gap_pixels: None,
            font_size: None,
            primary: false,
            primary_color: None,
            navigation: false,
            icon: None,
            icon_size: 16,
            striped: false,
            popup_position: None,
        }
    }
    /// A sidebar/tab destination; selection comes from the shared command state.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }
    pub fn icon_size(mut self, pixels: u16) -> Self {
        self.icon_size = pixels.clamp(8, 48);
        self
    }
    pub fn primary_color(mut self, color: crate::theme::color::Rgb) -> Self {
        self.primary = true;
        self.primary_color = Some(color);
        self
    }
    pub fn navigation(mut self) -> Self {
        self.navigation = true;
        self
    }
    /// Inset a pane, row or column without moving its outer border or hit area.
    pub fn padding_pixels(mut self, pixels: u16) -> Self {
        self.padding_pixels = pixels;
        self
    }
    /// Set physical group spacing; cell presentation rounds to its grid.
    pub fn gap_pixels(mut self, pixels: u16) -> Self {
        self.gap_pixels = Some(pixels);
        self
    }
    pub fn font_size(mut self, size: u16) -> Self {
        self.font_size = Some(size.clamp(6, 48));
        self
    }
    pub fn gap(mut self, gap: u16) -> Self {
        self.gap = Some(gap);
        self
    }
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
    pub fn striped(mut self) -> Self {
        self.striped = true;
        self
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
    /// Joined workflow destinations use the same commands, focus and hit handling
    /// as navigation. The application's command state marks the current phase.
    pub fn steps(id: impl Into<Id>, commands: &[Id]) -> Self {
        let id = id.into();
        Self::row(
            id.clone(),
            commands
                .iter()
                .map(|command| {
                    Self::button(format!("{}/{}", id.0, command.0), command.clone())
                        .navigation()
                        .height(Size::fill(1))
                })
                .collect(),
        )
        .height(Size::fixed(2))
        .gap_pixels(0)
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
    /// Anchor a modal menu beside its invocation point. Cells use cell units;
    /// graphics use viewport pixels. The solver keeps the entire menu visible.
    pub fn popup_at(mut self, x: u16, y: u16) -> Self {
        self.popup_position = Some((x, y));
        self
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
    #[test]
    fn action_colors_and_larger_icons_refresh_without_replacing_the_layout() {
        use crate::{native_surface::Primitive, theme::color::Rgb};
        let orange = Rgb::new(254, 128, 25);
        let alternate = Rgb::new(208, 135, 112);
        let view = |color| {
            View::new(
                Node::button("burn", "open")
                    .primary_color(color)
                    .icon("flame")
                    .icon_size(28)
                    .height(Size::fixed(2)),
            )
        };
        let mut ui = Runtime::new(view(orange), registry(), theme()).unwrap();
        ui.native_flat(true);
        let cells = ui.render_cells(Viewport::cells(40, 8)).unwrap();
        assert_eq!(cells[(0, 0)].bg, crate::chrome::rgb(orange));
        let surface = ui.render_surface(Viewport::graphics(320, 192)).unwrap();
        assert!(surface
            .nodes
            .iter()
            .any(|n| matches!(n, Primitive::Fill { color, .. } if color == &orange.to_hex())));
        assert!(surface.nodes.iter().any(
            |n| matches!(n, Primitive::Path { rect, .. } if rect.width == 28 && rect.height == 28)
        ));
        ui.update(view(alternate), registry()).unwrap();
        let surface = ui.render_surface(Viewport::graphics(320, 192)).unwrap();
        assert!(surface
            .nodes
            .iter()
            .any(|n| matches!(n, Primitive::Fill { color, .. } if color == &alternate.to_hex())));
        assert!(!surface
            .nodes
            .iter()
            .any(|n| matches!(n, Primitive::Fill { color, .. } if color == &orange.to_hex())));
    }
    #[test]
    fn compact_spacing_primary_actions_and_alternating_rows_are_opt_in() {
        let mut theme = theme();
        theme.row_bg = crate::theme::color::Rgb::new(40, 40, 40);
        theme.fg = crate::theme::color::Rgb::new(235, 219, 178);
        let view = View::new(
            Node::column(
                "root",
                vec![
                    Node::button("action", "open")
                        .primary()
                        .height(Size::fixed(1)),
                    Node::list(
                        "rows",
                        vec![Item::new("a", "Alpha"), Item::new("b", "Beta")],
                        None,
                    )
                    .striped(),
                ],
            )
            .gap(0),
        );
        let mut ui = Runtime::new(view.clone(), registry(), theme.clone()).unwrap();
        let cells = ui.render_cells(Viewport::cells(40, 8)).unwrap();
        assert_eq!(
            ui.current_frame()
                .unwrap()
                .get(&"rows".into())
                .unwrap()
                .area
                .y,
            1
        );
        assert_eq!(cells[(0, 0)].bg, crate::chrome::rgb(theme.accent));
        assert_ne!(cells[(0, 1)].bg, cells[(0, 2)].bg);
        ui.render_surface(Viewport::graphics(320, 192)).unwrap();
        let mut next = view;
        next.root.gap = Some(1);
        next.root.children[0].primary = false;
        next.root.children[1].striped = false;
        ui.update(next, registry()).unwrap();
        let cells = ui.render_cells(Viewport::cells(40, 8)).unwrap();
        assert_eq!(
            ui.current_frame()
                .unwrap()
                .get(&"rows".into())
                .unwrap()
                .area
                .y,
            2
        );
        assert_ne!(cells[(0, 0)].bg, crate::chrome::rgb(theme.accent));
        assert_eq!(cells[(0, 2)].bg, cells[(0, 3)].bg);
    }
    #[test]
    fn fleet_default_typography_is_shared_by_direct_and_mounted_surfaces() {
        let view = View::new(Node::pane(
            "pane",
            Heading::new("Workspace"),
            Node::column(
                "body",
                vec![
                    Node::label("heading", "Workspace")
                        .font_size(22)
                        .height(Size::fixed(2)),
                    Node::button("action", "open")
                        .primary()
                        .height(Size::fixed(1)),
                    Node::field("path", "/home"),
                ],
            ),
        ));
        let mut ui = Runtime::new(view, registry(), theme()).unwrap();
        ui.native_flat(true);
        let surface = ui.render_surface(Viewport::graphics(640, 480)).unwrap();
        assert!(!surface.ui_font);
        assert!(surface.nodes.iter().all(|n| !matches!(
            n,
            crate::native_surface::Primitive::Text { mono: false, .. }
        )));
        assert!(surface
            .nodes
            .iter()
            .any(|n| matches!(n, crate::native_surface::Primitive::Text { size: 22, .. })));
        assert!(surface
            .nodes
            .iter()
            .all(|n| !matches!(n, crate::native_surface::Primitive::Sprite { .. })));
        #[cfg(feature = "terminal-graphics")]
        {
            let scene = ui
                .render_scene(
                    crate::terminal_graphics::Viewport {
                        width: 640,
                        height: 480,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            for component in scene.components {
                if let crate::terminal_graphics::Component::Surface { surface, .. } = component {
                    assert!(!surface.ui_font);
                    assert!(surface.nodes.iter().all(|n| !matches!(
                        n,
                        crate::native_surface::Primitive::Text { mono: false, .. }
                    )));
                }
            }
        }
    }
    #[test]
    fn padding_and_pixel_gaps_keep_pointer_targets_in_the_painted_content() {
        for viewport in [Viewport::graphics(640, 480), Viewport::cells(80, 24)] {
            let view = View::new(
                Node::column(
                    "root",
                    vec![
                        Node::button("one", "open").height(Size::fixed(1)),
                        Node::button("two", "open").height(Size::fixed(1)),
                    ],
                )
                .padding_pixels(16)
                .gap_pixels(12),
            );
            let mut ui = Runtime::new(view, registry(), theme()).unwrap();
            let frame = ui.frame(viewport).unwrap().clone();
            let one = frame.get(&"one".into()).unwrap().area;
            let two = frame.get(&"two".into()).unwrap().area;
            assert!(one.x > 0 && one.y > 0);
            assert!(two.y > one.bottom());
            assert!(ui.pointer(0, 0, true).is_empty());
            assert!(ui.pointer(0, 0, false).is_empty());
            ui.pointer(two.x + 1, two.y, true);
            assert_eq!(
                ui.pointer(two.x + 1, two.y, false),
                vec![Event::Command("open".into())]
            );
            let layouts = ui.layout_count();
            let mut next = ui.view().clone();
            next.root.padding_pixels = 24;
            ui.update(next, registry()).unwrap();
            let after = ui.frame(viewport).unwrap();
            assert!(after.get(&"one".into()).unwrap().area.x > one.x);
            assert!(ui.layout_count() > layouts);
        }
    }
    #[test]
    fn field_insets_keep_line_height_and_buttons_honor_font_size() {
        let view = View::new(Node::column(
            "root",
            vec![
                Node::field("path", "/home/bstar")
                    .padding_pixels(10)
                    .height(Size::fixed(1)),
                Node::button("prepare", "open")
                    .font_size(15)
                    .height(Size::fixed(2)),
            ],
        ));
        let mut ui = Runtime::new(view, registry(), theme()).unwrap();
        ui.native_flat(true);
        let surface = ui.render_surface(Viewport::graphics(640, 300)).unwrap();
        let field = ui.current_frame().unwrap().get(&"path".into()).unwrap();
        assert_eq!(field.body.height, field.area.height);
        assert_eq!(field.body.x, field.area.x + 10);
        assert!(surface.nodes.iter().any(|n|matches!(n,crate::native_surface::Primitive::Text{rect,text,..} if text=="/home/bstar" && rect.x==field.area.x+10)));
        assert!(surface.nodes.iter().any(
            |n| matches!(n,crate::native_surface::Primitive::Text{text,size:15,..} if text=="Open")
        ));
    }
    #[test]
    fn flat_pane_focus_follows_the_control_and_icons_keep_labels_clear() {
        let view = View::new(Node::row(
            "root",
            vec![
                Node::pane(
                    "left",
                    Heading::new("Staging"),
                    Node::button("one", "open").icon("folder"),
                ),
                Node::pane(
                    "right",
                    Heading::new("Files"),
                    Node::button("two", "open").icon("archive"),
                ),
            ],
        ));
        let mut ui = Runtime::new(view, registry(), theme()).unwrap();
        ui.native_flat(true);
        for (control, pane, other) in [("one", "left", "right"), ("two", "right", "left")] {
            ui.request_focus(control.into()).unwrap();
            let surface = ui.render_surface(Viewport::graphics(800, 480)).unwrap();
            let frame = ui.current_frame().unwrap();
            let selected = frame.get(&pane.into()).unwrap().area;
            let inactive = frame.get(&other.into()).unwrap().area;
            let accent = ui.visual.colors.accent.clone();
            let has_outline = |area: crate::ratatui::layout::Rect| {
                surface.nodes.iter().any(|n| matches!(n,
                crate::native_surface::Primitive::Border { rect, color, .. } if rect.x == area.x && rect.y == area.y && rect.width == area.width && rect.height == area.height && color == &accent))
            };
            assert!(has_outline(selected));
            assert!(!has_outline(inactive));
            assert!(surface
                .nodes
                .iter()
                .any(|n| matches!(n, crate::native_surface::Primitive::Path { .. })));
        }
    }
    #[test]
    fn anchored_menus_clamp_at_edges_and_relocate_hit_targets() {
        for viewport in [Viewport::graphics(640, 480), Viewport::cells(80, 24)] {
            let menu = || {
                Node::menu("menu", Heading::new("Item"), &["open".into()])
                    .width(Size::fixed(20))
                    .height(Size::fixed(4))
            };
            let base = View::new(Node::label("base", "Workspace"));
            let mut ui = Runtime::new(
                base.clone().modal(menu().popup_at(u16::MAX, u16::MAX)),
                registry(),
                theme(),
            )
            .unwrap();
            let frame = ui.frame(viewport).unwrap().clone();
            let area = frame.get(&"menu".into()).unwrap().area;
            assert!(area.right() <= viewport.width && area.bottom() <= viewport.height);
            assert_eq!(area.right(), viewport.width);
            let layouts = ui.layout_count();
            ui.update(base.modal(menu().popup_at(1, 2)), registry())
                .unwrap();
            let next = ui.frame(viewport).unwrap();
            assert_eq!(next.get(&"menu".into()).unwrap().area.x, 1);
            assert!(ui.layout_count() > layouts);
        }
    }
    #[test]
    fn workflow_steps_fill_the_strip_and_typography_keeps_headlines() {
        let view = View::new(Node::column(
            "root",
            vec![
                Node::steps("steps", &["open".into(), "off".into()]),
                Node::label("title", "Headline")
                    .font_size(26)
                    .height(Size::fixed(2)),
            ],
        ));
        let mut ui = Runtime::new(view, registry(), theme()).unwrap();
        ui.native_font_size(14);
        let surface = ui.render_surface(Viewport::graphics(800, 600)).unwrap();
        assert!(surface.nodes.iter().any(|node| matches!(
            node,
            crate::native_surface::Primitive::Text { size: 26, .. }
        )));
        assert!(surface.nodes.iter().any(|node| matches!(
            node,
            crate::native_surface::Primitive::Text { size: 14, .. }
        )));
        let frame = ui.current_frame().unwrap();
        assert_eq!(
            frame.get(&"steps".into()).unwrap().area.height,
            frame.get(&"steps/open".into()).unwrap().area.height
        );
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
    #[test]
    fn opted_in_flat_panes_have_single_pixel_focused_and_inactive_edges() {
        let view = View::new(Node::row(
            "root",
            vec![
                Node::pane("left", Heading::new("Left"), Node::button("one", "open")),
                Node::pane("right", Heading::new("Right"), Node::button("two", "open")),
            ],
        ));
        let mut ui = Runtime::new(view, registry(), theme()).unwrap();
        ui.native_flat(true);
        ui.native_pane_border_pixels(1);
        for (control, pane) in [("one", "left"), ("two", "right")] {
            ui.request_focus(control.into()).unwrap();
            let surface = ui.render_surface(Viewport::graphics(800, 480)).unwrap();
            let frame = ui.current_frame().unwrap();
            for id in ["left", "right"] {
                let area = frame.get(&id.into()).unwrap().area;
                assert!(!surface.nodes.iter().any(|n|matches!(n,crate::native_surface::Primitive::Border{rect,..}if rect.x==area.x&&rect.y==area.y&&rect.width==area.width&&rect.height==area.height)));
                let color = if id == pane {
                    ui.visual.colors.accent.clone()
                } else {
                    ui.theme.border.to_hex()
                };
                assert!(surface.nodes.iter().any(|n|matches!(n,crate::native_surface::Primitive::Fill{rect,color:c,..}if rect.x==area.x&&rect.y==area.y&&rect.width==area.width&&rect.height==1&&c==&color)));
            }
        }
    }
}

#[cfg(test)]
mod control_tests;
