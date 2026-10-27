//! Opt-in raw input for an embedded canvas; ordinary application keys stay unchanged.
use super::protocol::{Component, Input, Scene};
use crate::crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPhase {
    Press,
    Repeat,
    Release,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbeddedKind {
    Key {
        code: String,
        phase: KeyPhase,
        modifiers: u8,
    },
    Pointer {
        action: String,
        button: u8,
        position: [f64; 2],
        modifiers: u8,
    },
    Text {
        text: String,
    },
    ReleaseAll,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedEvent {
    pub timestamp_ns: u64,
    pub sequence: u64,
    pub kind: EmbeddedKind,
}
pub fn monotonic_ns() -> u64 {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}
/// Hit testing for presentation policies uses the same foreground exclusions
/// as embedded input. Menus and floating controls always retain a visible cursor.
pub(super) fn canvas_at(scene: &Scene, point: [f64; 2]) -> Option<&str> {
    if scene
        .components
        .iter()
        .any(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
    {
        return None;
    }
    for (index, c) in scene.components.iter().enumerate().rev() {
        if matches!(c,Component::Surface{surface,..} if !surface.hits.is_empty())
            && super::pointer::surface_point(scene, index, point).is_some()
        {
            return None;
        }
        if let Component::LiveCanvas {
            id,
            view,
            interactive: true,
            ..
        } = c
        {
            if view.to_content(point, false).is_some() {
                return Some(id);
            }
        }
    }
    None
}
#[derive(Default)]
pub(crate) struct Router {
    active: Option<String>,
    pressed: u8,
    sequence: u64,
}
#[derive(Default)]
pub(crate) struct Routed {
    pub consumed: bool,
    pub input: Option<Input>,
    pub release: Option<Input>,
    pub keyboard: Option<bool>,
}
impl Router {
    pub fn active_canvas(&self) -> Option<&str> {
        self.active.as_deref()
    }
    /// Explicit local frontend request; never infer focus from a mouse position.
    pub fn focus_canvas(&mut self, canvas: &str, scene: &Scene) -> Routed {
        let allowed = !scene.components.iter().any(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
            && scene.components.iter().any(|c| matches!(c, Component::LiveCanvas { id, interactive: true, .. } if id == canvas));
        if !allowed {
            return Routed {
                release: self.release(),
                keyboard: Some(false),
                ..Default::default()
            };
        }
        let release = if self.active.as_deref() != Some(canvas) {
            self.release()
        } else {
            None
        };
        self.active = Some(canvas.into());
        Routed {
            release,
            keyboard: Some(true),
            ..Default::default()
        }
    }
    fn event(&mut self, canvas: String, kind: EmbeddedKind) -> Input {
        self.sequence = self.sequence.wrapping_add(1);
        Input::Embedded {
            canvas,
            event: EmbeddedEvent {
                timestamp_ns: monotonic_ns(),
                sequence: self.sequence,
                kind,
            },
        }
    }
    pub fn release(&mut self) -> Option<Input> {
        self.pressed = 0;
        self.active
            .take()
            .map(|id| self.event(id, EmbeddedKind::ReleaseAll))
    }
    pub fn route(&mut self, event: &Event, scene: &Scene, pixel_mouse: bool) -> Routed {
        let mut result = Routed {
            consumed: false,
            input: None,
            release: None,
            keyboard: None,
        };
        if scene
            .components
            .iter()
            .any(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
        {
            result.release = self.release();
            result.keyboard = Some(false);
            return result;
        }
        if self.active.as_ref().is_some_and(|active| {
            !scene
                .components
                .iter()
                .any(|c| matches!(c,Component::LiveCanvas{id,interactive:true,..} if id==active))
        }) {
            result.release = self.release();
            result.keyboard = Some(false);
        }
        if matches!(event, Event::FocusLost | Event::Resize(..)) {
            if let Some(release) = self.release() {
                result.release = Some(release);
            }
            if result.release.is_some() {
                result.keyboard = Some(false);
            }
            return result;
        }
        if let Event::Key(key) = event {
            if let Some(id) = self.active.clone() {
                result.consumed = true;
                if key.code == KeyCode::Esc && key.modifiers.contains(KeyModifiers::CONTROL) {
                    result.input = self.release();
                    result.keyboard = Some(false);
                } else {
                    result.input = Some(self.event(
                        id,
                        EmbeddedKind::Key {
                            code: super::client::key(key.code),
                            phase: match key.kind {
                                KeyEventKind::Press => KeyPhase::Press,
                                KeyEventKind::Repeat => KeyPhase::Repeat,
                                KeyEventKind::Release => KeyPhase::Release,
                            },
                            modifiers: key.modifiers.bits(),
                        },
                    ));
                }
            }
            return result;
        }
        if let Event::Paste(text) = event {
            if let Some(id) = self.active.clone() {
                result.consumed = true;
                result.input = Some(self.event(
                    id,
                    EmbeddedKind::Text {
                        text: text.chars().take(65536).collect(),
                    },
                ));
            }
            return result;
        }
        let Event::Mouse(mouse) = event else {
            return result;
        };
        let v = scene.viewport;
        let point = if pixel_mouse {
            [f64::from(mouse.column), f64::from(mouse.row)]
        } else {
            [
                (f64::from(mouse.column) + 0.5) * f64::from(v.width) / f64::from(v.columns.max(1)),
                (f64::from(mouse.row) + 0.5) * f64::from(v.height) / f64::from(v.rows.max(1)),
            ]
        };
        let mut canvas = None;
        for (component, c) in scene.components.iter().enumerate().rev() {
            if self.pressed == 0
                && matches!(c,Component::Surface{surface,..} if !surface.hits.is_empty())
                && super::pointer::surface_point(scene, component, point).is_some()
            {
                break;
            }
            if let Component::LiveCanvas {
                id,
                view,
                interactive: true,
                ..
            } = c
            {
                if (self.pressed != 0 && self.active.as_ref() == Some(id))
                    || view.to_content(point, false).is_some()
                {
                    canvas = Some((id.clone(), *view));
                    break;
                }
            }
        }
        let Some((id, view)) = canvas else {
            if matches!(mouse.kind, MouseEventKind::Down(_)) && self.active.is_some() {
                result.release = self.release();
                result.keyboard = Some(false);
            }
            return result;
        };
        let Some(position) = view.to_content(point, self.pressed != 0) else {
            return result;
        };
        let button = |b| match b {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        };
        let (action, b) = match mouse.kind {
            MouseEventKind::Down(b) => {
                if self.active.as_ref().is_some_and(|active| active != &id) {
                    result.release = self.release();
                }
                self.active = Some(id.clone());
                self.pressed |= 1 << button(b);
                result.keyboard = Some(true);
                ("down", button(b))
            }
            MouseEventKind::Up(b) => {
                self.pressed &= !(1 << button(b));
                ("up", button(b))
            }
            MouseEventKind::Drag(b) => ("drag", button(b)),
            MouseEventKind::Moved => ("move", 0),
            MouseEventKind::ScrollUp => ("scroll_up", 0),
            MouseEventKind::ScrollDown => ("scroll_down", 0),
            MouseEventKind::ScrollLeft => ("scroll_left", 0),
            MouseEventKind::ScrollRight => ("scroll_right", 0),
        };
        result.consumed = true;
        result.input = Some(self.event(
            id,
            EmbeddedKind::Pointer {
                action: action.into(),
                button: b,
                position,
                modifiers: mouse.modifiers.bits(),
            },
        ));
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floating_controls_and_menus_take_input_above_live_apps() {
        use crate::crossterm::event::MouseEvent;
        use crate::native_surface::{style::Style, PixelRect};
        let viewport = super::super::protocol::Viewport::default();
        let style = Style::default().with_soft_workspace();
        let mut scene = style.scene(viewport);
        scene.components.push(Component::LiveCanvas {
            rect: crate::ratatui::layout::Rect::new(0, 0, viewport.columns, viewport.rows).into(),
            id: "desktop".into(),
            view: crate::content_view::ContentView::new(
                [1920, 1080],
                PixelRect::new(0, 0, viewport.width as u16, viewport.height as u16),
            )
            .unwrap(),
            interactive: true,
        });
        let mut dock = style.floating_panel(440, 54);
        style.tool(
            &mut dock,
            PixelRect::new(8, 8, 100, 34),
            "Apps",
            "launch",
            false,
        );
        style.mount(&mut scene, dock, 100, 100).unwrap();
        let mouse = |x, y| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert_eq!(canvas_at(&scene, [120., 120.]), None);
        assert_eq!(canvas_at(&scene, [600., 400.]), Some("desktop"));
        let mut router = Router::default();
        assert!(
            !router.route(&mouse(120, 120), &scene, true).consumed,
            "Dock must receive its own click"
        );
        assert!(router.route(&mouse(600, 400), &scene, true).consumed);
        scene.components.push(Component::Menu {
            rect: crate::ratatui::layout::Rect::new(0, 0, 30, 10).into(),
        });
        let key = Event::Key(crate::crossterm::event::KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ));
        assert_eq!(canvas_at(&scene, [600., 400.]), None);
        let routed = router.route(&key, &scene, true);
        assert!(!routed.consumed && routed.release.is_some());
        assert!(router.active.is_none());
        assert!(!router.route(&mouse(600, 400), &scene, true).consumed);
    }
    #[test]
    fn letterbox_drag_focus_loss_and_toolbar_release_are_balanced() {
        use crate::crossterm::event::MouseEvent;
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                0, 0, 100, 40,
            )),
            super::super::protocol::Viewport {
                width: 1000,
                height: 1000,
                ..Default::default()
            },
            1,
        );
        scene.components.push(Component::LiveCanvas {
            rect: crate::ratatui::layout::Rect::new(0, 0, 100, 40).into(),
            id: "demo".into(),
            view: crate::content_view::ContentView::new(
                [1920, 1080],
                crate::native_surface::PixelRect::new(100, 100, 800, 800),
            )
            .unwrap(),
            interactive: true,
        });
        let mouse = |kind, x, y| {
            Event::Mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        let mut r = Router::default();
        assert!(
            !r.route(
                &mouse(MouseEventKind::Down(MouseButton::Left), 110, 110),
                &scene,
                true
            )
            .consumed
        );
        assert!(
            r.route(
                &mouse(MouseEventKind::Down(MouseButton::Left), 500, 500),
                &scene,
                true
            )
            .consumed
        );
        let drag = r.route(
            &mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0),
            &scene,
            true,
        );
        assert!(matches!(
            drag.input,
            Some(Input::Embedded {
                event: EmbeddedEvent {
                    kind: EmbeddedKind::Pointer {
                        position: [0., 0.],
                        ..
                    },
                    ..
                },
                ..
            })
        ));
        r.route(
            &mouse(MouseEventKind::Up(MouseButton::Left), 0, 0),
            &scene,
            true,
        );
        let toolbar = r.route(
            &mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &scene,
            true,
        );
        assert!(!toolbar.consumed && toolbar.release.is_some());
        r.route(
            &mouse(MouseEventKind::Down(MouseButton::Left), 500, 500),
            &scene,
            true,
        );
        assert!(r.route(&Event::FocusLost, &scene, true).release.is_some());
        assert!(r.active.is_none());
    }
    #[test]
    fn explicit_window_activation_restores_capture_without_a_pointer_click() {
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                0, 0, 100, 40,
            )),
            Default::default(),
            1,
        );
        scene.components.push(Component::LiveCanvas {
            rect: crate::ratatui::layout::Rect::new(0, 0, 100, 40).into(),
            id: "terminal".into(),
            view: crate::content_view::ContentView::new(
                [1000, 600],
                crate::native_surface::PixelRect::new(0, 0, 1000, 600),
            )
            .unwrap(),
            interactive: true,
        });
        let mut router = Router::default();
        // Ordinary applications still require their own explicit focus policy.
        assert_eq!(
            router.route(&Event::FocusGained, &scene, true).keyboard,
            None
        );
        assert!(router.active.is_none());
        assert_eq!(router.focus_canvas("terminal", &scene).keyboard, Some(true));
        assert!(router
            .route(&Event::FocusLost, &scene, true)
            .release
            .is_some());
        assert!(router.active.is_none());
        assert_eq!(router.focus_canvas("terminal", &scene).keyboard, Some(true));
        let key = crate::crossterm::event::KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(
            router.route(&Event::Key(key), &scene, true).input,
            Some(Input::Embedded {
                event: EmbeddedEvent {
                    kind: EmbeddedKind::Key {
                        phase: KeyPhase::Press,
                        ..
                    },
                    ..
                },
                ..
            })
        ));
        assert_eq!(router.focus_canvas("missing", &scene).keyboard, Some(false));
        assert!(router.active.is_none());
        if let Component::LiveCanvas { interactive, .. } = &mut scene.components[0] {
            *interactive = false;
        }
        assert_eq!(
            router.focus_canvas("terminal", &scene).keyboard,
            Some(false)
        );
        if let Component::LiveCanvas { interactive, .. } = &mut scene.components[0] {
            *interactive = true;
        }
        scene.components.push(Component::Dialog {
            rect: crate::ratatui::layout::Rect::new(0, 0, 30, 10).into(),
            title: "Rename".into(),
        });
        assert_eq!(
            router.focus_canvas("terminal", &scene).keyboard,
            Some(false)
        );
        assert!(router.active.is_none());
    }
    #[test]
    fn key_phases_and_escape_balance_capture() {
        let mut r = Router {
            active: Some("demo".into()),
            ..Default::default()
        };
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                0, 0, 100, 40,
            )),
            Default::default(),
            1,
        );
        scene.components.push(Component::LiveCanvas {
            rect: crate::ratatui::layout::Rect::new(0, 0, 100, 40).into(),
            id: "demo".into(),
            view: crate::content_view::ContentView::new(
                [1920, 1080],
                crate::native_surface::PixelRect::new(0, 0, 1000, 600),
            )
            .unwrap(),
            interactive: true,
        });
        let key = crate::crossterm::event::KeyEvent::new_with_kind(
            KeyCode::Char('w'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(matches!(
            r.route(&Event::Key(key), &scene, true).input,
            Some(Input::Embedded {
                event: EmbeddedEvent {
                    kind: EmbeddedKind::Key {
                        phase: KeyPhase::Release,
                        ..
                    },
                    ..
                },
                ..
            })
        ));
        let escape = Event::Key(crate::crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::CONTROL,
        ));
        assert!(matches!(
            r.route(&escape, &scene, true).input,
            Some(Input::Embedded {
                event: EmbeddedEvent {
                    kind: EmbeddedKind::ReleaseAll,
                    ..
                },
                ..
            })
        ));
        assert!(r.active.is_none());
    }
}
