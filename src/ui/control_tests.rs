use super::*;
use crate::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    native_surface::Primitive,
    theme::{Theme, ThemeFile, BUILTINS},
};
use command::{Command, Registry};

fn theme() -> Theme {
    Theme::resolve(&ThemeFile::parse(BUILTINS[0].toml).unwrap())
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn list(selected: Option<Id>) -> View {
    let mut items: Vec<_> = (0..20)
        .map(|i| Item::new(format!("row-{i}"), format!("Row {i}")))
        .collect();
    items[4].enabled = false;
    View::new(Node::table("list", vec!["Title".into()], items, selected))
}
fn selected(index: usize) -> Vec<Event> {
    vec![Event::Selected {
        id: "list".into(),
        item: format!("row-{index}").into(),
    }]
}
fn viewports() -> [Viewport; 2] {
    [Viewport::cells(40, 5), Viewport::graphics(336, 136)]
}

#[test]
fn explicit_focus_survives_hiding_and_modal_input_stays_trapped() {
    let view = |hidden| {
        View::new(Node::column(
            "root",
            vec![
                Node::field("first", ""),
                Node::field("second", "").hidden(hidden),
            ],
        ))
    };
    let mut ui = Runtime::new(view(false), Registry::default(), theme()).unwrap();
    assert!(ui.request_focus("second".into()).unwrap());
    let viewport = Viewport::cells(80, 24);
    assert_eq!(ui.frame(viewport).unwrap().focus, Some("second".into()));
    ui.set_view(view(true)).unwrap();
    assert_eq!(ui.frame(viewport).unwrap().focus, Some("first".into()));
    ui.set_view(view(false)).unwrap();
    assert_eq!(ui.frame(viewport).unwrap().focus, Some("second".into()));
    ui.set_view(view(false).modal(Node::field("dialog", "")))
        .unwrap();
    ui.request_focus("first".into()).unwrap();
    assert_eq!(ui.frame(viewport).unwrap().focus, Some("dialog".into()));
    assert!(ui.request_focus("missing".into()).is_err());
    assert!(ui.request_focus("root".into()).is_err());
    assert_eq!(
        ui.key(key(KeyCode::Char('x'))),
        vec![Event::Changed {
            id: "dialog".into(),
            value: "x".into()
        }]
    );
    ui.set_view(view(false)).unwrap();
    assert_eq!(ui.frame(viewport).unwrap().focus, Some("second".into()));
}

#[test]
fn inline_editor_cancellation_reports_its_owner_and_retains_the_draft() {
    let mut ui = Runtime::new(
        View::new(Node::field("filter", "Draft")),
        Registry::default(),
        theme(),
    )
    .unwrap();
    ui.frame(Viewport::cells(40, 1)).unwrap();
    assert_eq!(
        ui.key(key(KeyCode::Esc)),
        vec![Event::Cancelled {
            id: "filter".into()
        }]
    );
    assert_eq!(ui.fields[&Id::from("filter")].text(), "Draft");
}

#[test]
fn tab_and_pointer_report_user_focus_before_content_input() {
    let mut ui = Runtime::new(
        View::new(Node::row(
            "root",
            vec![
                Node::field("first", ""),
                Node::new(
                    "content",
                    Widget::Custom {
                        fallback: "Article".into(),
                        revision: 0,
                    },
                ),
            ],
        )),
        Registry::default(),
        theme(),
    )
    .unwrap();
    let viewport = Viewport::cells(80, 24);
    ui.frame(viewport).unwrap();
    assert_eq!(
        ui.key(key(KeyCode::Tab)),
        vec![Event::Focused("content".into())]
    );
    assert_eq!(
        ui.key(key(KeyCode::BackTab)),
        vec![Event::Focused("first".into())]
    );
    let area = ui
        .frame(viewport)
        .unwrap()
        .get(&"content".into())
        .unwrap()
        .body;
    assert_eq!(
        ui.pointer(area.x + 2, area.y + 3, true),
        vec![
            Event::Focused("content".into()),
            Event::CustomPointer {
                id: "content".into(),
                x: 2,
                y: 3,
                pressed: true
            },
        ]
    );
    assert_eq!(
        ui.scroll(area.x + 2, area.y + 3, 3),
        vec![Event::CustomScroll {
            id: "content".into(),
            x: 2,
            y: 3,
            lines: 3
        }]
    );
    assert!(ui.scroll(area.x + 2, area.y + 3, 0).is_empty());
    assert_eq!(ui.layout_count(), 1);
}

#[cfg(all(feature = "terminal-graphics", unix))]
#[test]
fn presentation_switch_is_available_inside_modal_fields_without_losing_their_value() {
    let view =
        View::new(Node::field("background", "Background")).modal(Node::field("dialog", "Draft"));
    let mut ui = Runtime::new(view, Registry::new(command::standard()).unwrap(), theme()).unwrap();
    ui.render_surface(Viewport::graphics(800, 600)).unwrap();
    assert_eq!(
        ui.key(key(KeyCode::F(9))),
        vec![Event::Command("kit.presentation".into())]
    );
    ui.render_cells(Viewport::cells(100, 40)).unwrap();
    assert_eq!(ui.current_frame().unwrap().focus, Some("dialog".into()));
    assert_eq!(ui.fields[&Id::from("dialog")].text(), "Draft");
    assert_eq!(
        ui.key(key(KeyCode::Char('x'))),
        vec![Event::Changed {
            id: "dialog".into(),
            value: "Draftx".into()
        }]
    );
    assert_eq!(
        ui.key(key(KeyCode::F(9))),
        vec![Event::Command("kit.presentation".into())]
    );
    ui.render_surface(Viewport::graphics(800, 600)).unwrap();
    assert_eq!(ui.fields[&Id::from("dialog")].text(), "Draftx");
}

fn table(width: u16, align: Align) -> View {
    View::new(Node::table_with_columns(
        "list",
        vec![
            Column::new("Title"),
            Column::new("Count").width(Size::fixed(width)).align(align),
        ],
        (0..20)
            .map(|i| Item {
                columns: vec![format!("Row {i}"), (i + 1).to_string()],
                ..Item::new(format!("row-{i}"), "")
            })
            .collect(),
        Some("row-0".into()),
    ))
}

#[test]
fn table_columns_align_headings_values_and_scrolled_pointer_rows() {
    for viewport in viewports() {
        let mut ui = Runtime::new(table(6, Align::Right), Registry::default(), theme()).unwrap();
        let p = ui
            .frame(viewport)
            .unwrap()
            .get(&"list".into())
            .unwrap()
            .clone();
        let count = &p.columns[1].area;
        assert_eq!(
            count.width,
            if viewport.presentation == Presentation::Cells {
                6
            } else {
                48
            }
        );
        let rh = if viewport.presentation == Presentation::Cells {
            1
        } else {
            24
        };
        ui.scroll(count.x, count.y + rh, 8);
        assert_eq!(
            ui.frame(viewport)
                .unwrap()
                .get(&"list".into())
                .unwrap()
                .columns,
            p.columns
        );
        if viewport.presentation == Presentation::Cells {
            let buffer = ui.render_cells(viewport).unwrap();
            assert_eq!(buffer[(count.right() - 1, count.y)].symbol(), "t");
            assert_eq!(buffer[(count.right() - 1, count.y + rh)].symbol(), "9");
            assert_eq!(buffer[(count.right() - 1, count.y + 2 * rh)].symbol(), "0");
            assert_eq!(buffer[(count.right() - 2, count.y + 2 * rh)].symbol(), "1");
        } else {
            let surface = ui.render_surface(viewport).unwrap();
            surface.validate().unwrap();
            for value in ["Count", "9", "10"] {
                let (index, node) = surface
                    .nodes
                    .iter()
                    .enumerate()
                    .find(|(_, node)| {
                        matches!(node,
                    Primitive::Text { text, .. } if text == value)
                    })
                    .unwrap();
                assert_eq!(node.rect().x, count.x);
                assert_eq!(node.rect().width, count.width);
                assert!(surface.right_aligned_text.contains(&(index as u16)));
            }
        }
        ui.pointer(count.right() - 1, count.y + rh, true);
        assert_eq!(
            ui.pointer(count.right() - 1, count.y + rh, false),
            selected(8)
        );
        assert_eq!(ui.layout_count(), 1);
    }
}

#[test]
fn table_width_changes_cancel_capture_but_alignment_and_heading_updates_reuse_geometry() {
    let viewport = Viewport::cells(40, 5);
    let mut ui = Runtime::new(table(6, Align::Right), Registry::default(), theme()).unwrap();
    ui.frame(viewport).unwrap();
    let mut changed = table(6, Align::Center);
    changed.root.table_columns[1].heading = "Total".into();
    if let Widget::List { columns, .. } = &mut changed.root.widget {
        columns[1] = "Total".into();
    }
    ui.set_view(changed).unwrap();
    let p = ui
        .frame(viewport)
        .unwrap()
        .get(&"list".into())
        .unwrap()
        .clone();
    assert_eq!(p.columns[1].align, Align::Center);
    let buffer = ui.render_cells(viewport).unwrap();
    assert_eq!(
        buffer[(p.columns[1].area.x + 2, p.body.y + 1)].symbol(),
        "1"
    );
    assert_eq!(ui.layout_count(), 1);
    ui.pointer(p.body.x, p.body.y + 1, true);
    ui.set_view(table(10, Align::Right)).unwrap();
    ui.frame(viewport).unwrap();
    assert!(ui.pointer(p.body.x, p.body.y + 1, false).is_empty());
    assert!(ui.captured().is_none());
    assert_eq!(ui.layout_count(), 2);
}

#[test]
fn table_clipping_preserves_clusters_and_the_neighboring_column() {
    let mut view = table(6, Align::Right);
    view.root.table_columns[0].width = Size::fixed(4);
    if let Widget::List { items, .. } = &mut view.root.widget {
        let items = std::sync::Arc::make_mut(items);
        items[0].columns[0] = "a\u{301}日本語\x1b\n".into();
        items[1].columns[0] = "👩‍💻abc".into();
    }
    let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
    let buffer = ui.render_cells(Viewport::cells(11, 4)).unwrap();
    assert_eq!(buffer[(0, 1)].symbol(), "a\u{301}");
    assert_eq!(buffer[(1, 1)].symbol(), "日");
    assert_eq!(buffer[(3, 1)].symbol(), "…");
    assert_eq!(buffer[(0, 2)].symbol(), "👩‍💻");
    assert_eq!(buffer[(2, 2)].symbol(), "a");
    assert_eq!(buffer[(3, 2)].symbol(), "…");
    assert_eq!(buffer[(4, 1)].symbol(), " ");
    assert_eq!(buffer[(10, 1)].symbol(), "1");
}

#[test]
fn table_columns_stay_bounded_in_tiny_viewports_and_legacy_tables_remain_supported() {
    for width in 1..=64 {
        for viewport in [
            Viewport::cells(width, 5),
            Viewport::graphics(width * 8, 136),
        ] {
            let mut ui =
                Runtime::new(table(6, Align::Right), Registry::default(), theme()).unwrap();
            let frame = ui.frame(viewport).unwrap();
            if let Some(p) = frame.get(&"list".into()) {
                for column in &p.columns {
                    assert!(column.area.x >= p.body.x);
                    assert!(column.area.right() <= p.body.right());
                }
                for columns in p.columns.windows(2) {
                    assert!(columns[0].area.right() <= columns[1].area.x);
                }
            }
            if viewport.presentation == Presentation::Graphics {
                ui.render_surface(viewport).unwrap().validate().unwrap();
            } else {
                ui.render_cells(viewport).unwrap();
            }
        }
    }
    let mut view = table(6, Align::Right);
    view.root.table_columns.clear();
    let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
    let columns = &ui
        .frame(Viewport::cells(21, 5))
        .unwrap()
        .get(&"list".into())
        .unwrap()
        .columns;
    assert_eq!((columns[0].area.width, columns[1].area.width), (10, 10));
}

#[test]
fn table_specifications_reject_mismatched_headings_and_invalid_sizes() {
    let mut view = table(6, Align::Right);
    view.root.table_columns[1].heading = "Wrong".into();
    assert!(Runtime::new(view, Registry::default(), theme()).is_err());
    let mut view = table(6, Align::Right);
    view.root.table_columns[1].width = Size::bounded(10, 6, 5);
    assert!(Runtime::new(view, Registry::default(), theme()).is_err());
    let mut view = table(6, Align::Right);
    view.root.widget = Widget::Label("Wrong widget".into());
    assert!(Runtime::new(view, Registry::default(), theme()).is_err());
}

#[cfg(feature = "terminal-graphics")]
#[test]
fn modal_table_columns_are_local_to_their_surface_while_pointer_geometry_stays_global() {
    use crate::terminal_graphics::{Component, Viewport as NativeViewport};
    let view = View::new(Node::label("background", "Background")).modal(
        Node::pane("dialog", Heading::new("Table"), table(6, Align::Right).root)
            .width(Size::fixed(40))
            .height(Size::fixed(12)),
    );
    let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
    let viewport = NativeViewport {
        width: 800,
        height: 600,
        ..Default::default()
    };
    let scene = ui.render_scene(viewport, false).unwrap();
    let frame = ui.current_frame().unwrap();
    let dialog = frame.get(&"dialog".into()).unwrap().area;
    let count = frame.get(&"list".into()).unwrap().columns[1].area;
    let surface = scene
        .components
        .iter()
        .find_map(|component| match component {
            Component::Surface { surface, .. } if surface.width == dialog.width => Some(surface),
            _ => None,
        })
        .unwrap();
    surface.validate().unwrap();
    let heading = surface
        .nodes
        .iter()
        .find(|node| {
            matches!(node,
        Primitive::Text { text, .. } if text == "Count")
        })
        .unwrap();
    assert_eq!(
        (heading.rect().x, heading.rect().y),
        (count.x - dialog.x, count.y - dialog.y)
    );
    ui.pointer(count.right() - 1, count.y + 24, true);
    assert_eq!(
        ui.pointer(count.right() - 1, count.y + 24, false),
        selected(0)
    );
}

#[test]
fn multiline_navigation_follows_the_current_presentation_before_paint() {
    let text = "abcdefghijklmnopqrst\nsecond\nlast";
    let mut ui = Runtime::new(
        View::new(Node::multiline("composer", text)),
        Registry::default(),
        theme(),
    )
    .unwrap();
    // Establish a narrow cell layout, then switch to native hard lines. The
    // cell renderer's old wrap width must not remain in the native editor.
    ui.render_cells(Viewport::cells(10, 3)).unwrap();
    ui.frame(Viewport::graphics(400, 120)).unwrap();
    ui.key(key(KeyCode::Up));
    ui.key(key(KeyCode::Up));
    assert_eq!(ui.fields[&"composer".into()].cursor(), 4);

    // Switching back must install the new wrap width before any input, even
    // if a paint is delayed by transport backpressure.
    ui.fields
        .get_mut(&"composer".into())
        .unwrap()
        .set_cursor(text.len());
    ui.frame(Viewport::cells(10, 3)).unwrap();
    ui.key(key(KeyCode::Up));
    ui.key(key(KeyCode::Up));
    assert_eq!(ui.fields[&"composer".into()].cursor(), 14);
    assert_eq!(
        ui.key(key(KeyCode::Enter)),
        vec![Event::Submitted {
            id: "composer".into(),
            value: text.into(),
        }]
    );
    assert_eq!(
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        vec![Event::Changed {
            id: "composer".into(),
            value: "abcdefghijklmn\nopqrst\nsecond\nlast".into()
        }]
    );
}

#[test]
fn password_cells_scroll_and_show_a_caret_without_exposing_text() {
    let mut ui = Runtime::new(
        View::new(Node::password("password", "秘密abcd")),
        Registry::default(),
        theme(),
    )
    .unwrap();
    let viewport = Viewport::cells(4, 1);
    let buffer = ui.render_cells(viewport).unwrap();
    assert!(buffer
        .content
        .iter()
        .all(|cell| matches!(cell.symbol(), "*" | " ")));
    assert!(buffer[(3, 0)]
        .modifier
        .contains(crate::ratatui::style::Modifier::REVERSED));
    ui.key(key(KeyCode::Home));
    let buffer = ui.render_cells(viewport).unwrap();
    assert!(buffer[(0, 0)]
        .modifier
        .contains(crate::ratatui::style::Modifier::REVERSED));
    let surface = ui.render_surface(Viewport::graphics(200, 48)).unwrap();
    surface.validate().unwrap();
    assert_eq!(surface.text_carets.len(), 1);
    assert_eq!(surface.text_carets[0].byte, 0);
    assert!(!serde_json::to_string(&surface).unwrap().contains("秘密"));
    assert_eq!(ui.fields[&"password".into()].text(), "秘密abcd");
}

#[test]
fn non_caret_native_lines_keep_their_beginning_when_the_value_is_large() {
    let text = format!("FIRST{}\nlast", "a".repeat(4096));
    let mut ui = Runtime::new(
        View::new(Node::multiline("composer", text)),
        Registry::default(),
        theme(),
    )
    .unwrap();
    let surface = ui.render_surface(Viewport::graphics(400, 120)).unwrap();
    surface.validate().unwrap();
    assert!(surface
        .nodes
        .iter()
        .any(|node| matches!(node, Primitive::Text { text, .. } if text.starts_with("FIRST"))));
}

#[test]
fn list_navigation_uses_visible_data_rows_and_skips_disabled_items() {
    // Both viewports leave four data rows after the table header.
    for viewport in viewports() {
        let mut ui = Runtime::new(list(None), Registry::default(), theme()).unwrap();
        ui.frame(viewport).unwrap();
        assert_eq!(ui.key(key(KeyCode::Down)), selected(0));
        assert_eq!(ui.key(key(KeyCode::PageDown)), selected(5));
        assert_eq!(ui.key(key(KeyCode::PageDown)), selected(9));
        assert_eq!(ui.key(key(KeyCode::PageUp)), selected(5));
        assert_eq!(ui.key(key(KeyCode::Up)), selected(3));
        assert_eq!(ui.key(key(KeyCode::End)), selected(19));
        assert_eq!(ui.key(key(KeyCode::Home)), selected(0));
        assert_eq!(
            ui.key(key(KeyCode::Enter)),
            vec![Event::Activated {
                id: "list".into(),
                item: "row-0".into(),
            }]
        );
    }
}

#[test]
fn modified_navigation_reaches_registered_shortcuts_in_lists_and_sliders() {
    let commands = Registry::new(vec![
        Command::new("move", "Move").shortcut("ctrl+right"),
        Command::new("last", "Last").shortcut("alt+end"),
    ])
    .unwrap();
    for view in [
        View::new(Node::slider("slider", 40, 100)),
        list(Some("row-2".into())),
    ] {
        let mut ui = Runtime::new(view, commands.clone(), theme()).unwrap();
        ui.frame(Viewport::cells(40, 10)).unwrap();
        assert_eq!(
            ui.key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL)),
            vec![Event::Command("move".into())]
        );
        assert_eq!(
            ui.key(KeyEvent::new(KeyCode::End, KeyModifiers::ALT)),
            vec![Event::Command("last".into())]
        );
    }
}

#[test]
fn slider_finishes_its_capture_when_released_outside_the_control() {
    for viewport in viewports() {
        let mut ui = Runtime::new(
            View::new(Node::slider("slider", 40, 100)),
            Registry::default(),
            theme(),
        )
        .unwrap();
        let area = ui
            .frame(viewport)
            .unwrap()
            .get(&"slider".into())
            .unwrap()
            .area;
        ui.pointer(area.x, area.y, true);
        assert_eq!(
            ui.pointer(u16::MAX, u16::MAX, false),
            vec![Event::Adjusted {
                id: "slider".into(),
                value: 100,
                finished: true,
            }]
        );
        assert!(ui.captured().is_none());
        assert!(ui.pointer(0, 0, false).is_empty());
    }
}

#[test]
fn wheel_scroll_cancels_a_click_without_snapping_back_to_selection() {
    for viewport in viewports() {
        let mut ui =
            Runtime::new(list(Some("row-0".into())), Registry::default(), theme()).unwrap();
        let p = ui
            .frame(viewport)
            .unwrap()
            .get(&"list".into())
            .unwrap()
            .clone();
        let row_height = if viewport.presentation == Presentation::Cells {
            1
        } else {
            24
        };
        let y = p.body.y + row_height;
        ui.pointer(p.body.x, y, true);
        ui.scroll(p.body.x, y, 8);
        assert!(ui.pointer(p.body.x, y, false).is_empty());
        // Input can arrive before either renderer has run again.
        ui.pointer(p.body.x, y, true);
        assert_eq!(ui.pointer(p.body.x, y, false), selected(8));
    }
}

#[test]
fn external_selection_and_reordering_share_scroll_with_rendering_and_hits() {
    for viewport in viewports() {
        let mut ui =
            Runtime::new(list(Some("row-0".into())), Registry::default(), theme()).unwrap();
        ui.frame(viewport).unwrap();
        ui.set_view(list(Some("row-15".into()))).unwrap();
        let p = ui
            .frame(viewport)
            .unwrap()
            .get(&"list".into())
            .unwrap()
            .clone();
        let row_height = if viewport.presentation == Presentation::Cells {
            1
        } else {
            24
        };
        // Selection 15 is the fourth visible data row, so row 12 is first.
        ui.pointer(p.body.x, p.body.y + row_height, true);
        assert_eq!(
            ui.pointer(p.body.x, p.body.y + row_height, false),
            selected(12)
        );
        let mut changed = list(Some("row-12".into()));
        if let Widget::List { items, .. } = &mut changed.root.widget {
            std::sync::Arc::make_mut(items).reverse();
        }
        ui.set_view(changed).unwrap();
        ui.frame(viewport).unwrap();
        // Selected row 12 now has index 7 and is the first visible row.
        ui.pointer(p.body.x, p.body.y + row_height, true);
        assert_eq!(
            ui.pointer(p.body.x, p.body.y + row_height, false),
            selected(12)
        );
        assert_eq!(ui.layout_count(), 1);
        if viewport.presentation == Presentation::Cells {
            let buffer = ui.render_cells(viewport).unwrap();
            assert_eq!(buffer[(p.body.x, p.body.y + row_height)].symbol(), "R");
        } else {
            let surface = ui.render_surface(viewport).unwrap();
            assert!(surface.nodes.iter().any(|node| matches!(node,
                Primitive::Text { text, .. } if text == "Row 12")));
        }
    }
}

#[test]
fn disabling_a_field_updates_focus_before_paste_or_pointer_input() {
    let view = |enabled| {
        View::new(Node::column(
            "root",
            vec![
                Node::field("first", "").enabled(enabled),
                Node::field("second", ""),
            ],
        ))
    };
    let mut ui = Runtime::new(view(true), Registry::default(), theme()).unwrap();
    ui.frame(Viewport::cells(40, 10)).unwrap();
    ui.set_view(view(false)).unwrap();
    assert_eq!(
        ui.paste("safe"),
        vec![Event::Changed {
            id: "second".into(),
            value: "safe".into(),
        }]
    );
    assert_eq!(ui.current_frame().unwrap().focus, Some("second".into()));
}

#[test]
fn empty_and_disabled_lists_do_not_activate_or_select_rows() {
    for mut view in [list(None), View::new(Node::list("list", vec![], None))] {
        if let Widget::List { items, .. } = &mut view.root.widget {
            for item in std::sync::Arc::make_mut(items) {
                item.enabled = false;
            }
        }
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        ui.frame(Viewport::cells(40, 5)).unwrap();
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Enter,
        ] {
            assert!(ui.key(key(code)).is_empty());
        }
    }
}

#[test]
fn external_selection_cancels_a_click_if_it_scrolls_the_pressed_row() {
    for viewport in viewports() {
        let mut ui =
            Runtime::new(list(Some("row-0".into())), Registry::default(), theme()).unwrap();
        let p = ui
            .frame(viewport)
            .unwrap()
            .get(&"list".into())
            .unwrap()
            .clone();
        let y = p.body.y
            + if viewport.presentation == Presentation::Cells {
                1
            } else {
                24
            };
        ui.pointer(p.body.x, y, true);
        ui.set_view(list(Some("row-15".into()))).unwrap();
        assert!(ui.pointer(p.body.x, y, false).is_empty());
        assert!(ui.captured().is_none());
    }
}

#[test]
fn disabling_a_captured_slider_cancels_its_release() {
    let mut ui = Runtime::new(
        View::new(Node::slider("slider", 40, 100)),
        Registry::default(),
        theme(),
    )
    .unwrap();
    ui.frame(Viewport::cells(40, 5)).unwrap();
    ui.pointer(10, 0, true);
    ui.set_view(View::new(Node::slider("slider", 40, 100).enabled(false)))
        .unwrap();
    assert!(ui.pointer(39, 0, false).is_empty());
    assert!(ui.captured().is_none());
}

#[test]
fn modal_shell_has_focus_border_even_without_focusable_children() {
    for child in [Node::label("notice", "Read only"), Node::field("entry", "")] {
        let view = View::new(Node::pane(
            "background",
            Heading::new("Background"),
            Node::field("background-field", ""),
        ))
        .modal(Node::pane("dialog", Heading::new("Dialog"), child));
        let mut ui = Runtime::new(view, Registry::default(), theme()).unwrap();
        let buffer = ui.render_cells(Viewport::cells(80, 24)).unwrap();
        let frame = ui.current_frame().unwrap();
        let modal = frame.get(&"dialog".into()).unwrap();
        assert_eq!(
            buffer[(modal.area.x, modal.area.y)].fg,
            crate::chrome::rgb(ui.theme().border_focused)
        );
        let background = frame.get(&"background".into()).unwrap();
        assert_eq!(
            buffer[(background.area.x, background.area.y)].fg,
            crate::chrome::rgb(ui.theme().border)
        );
        let surface = ui.render_surface(Viewport::graphics(800, 600)).unwrap();
        let frame = ui.current_frame().unwrap();
        let area = frame.get(&"dialog".into()).unwrap().area;
        let accent = crate::native_surface::style::Style::new(
            crate::native_surface::style::Palette::for_chrome(ui.theme()),
        )
        .colors
        .accent;
        assert!(surface.nodes.iter().any(|node| matches!(node,
            Primitive::Border { rect, color, .. } if *rect == crate::native_surface::PixelRect::new(area.x, area.y, area.width, area.height) && color == &accent)));
        let area = frame.get(&"background".into()).unwrap().area;
        assert!(!surface.nodes.iter().any(|node| matches!(node,
            Primitive::Border { rect, color, .. } if *rect == crate::native_surface::PixelRect::new(area.x, area.y, area.width, area.height) && color == &accent)));
    }
}
