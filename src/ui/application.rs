//! Adapter from the application UI contract to KIT's persistent local/SSH host.
use super::{
    command::{Command, Registry},
    Event, Frame, Runtime, View,
};
use crate::crossterm::event::{KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use crate::theme::Theme;
#[cfg(all(feature = "terminal-graphics", unix))]
use crate::{
    crossterm::event::{KeyCode, KeyModifiers},
    terminal_graphics::{
        capabilities::Capabilities,
        protocol::{Input, Scene, ServerMessage, Viewport},
        session::Controller,
    },
};

/// Application state and domain work. KIT owns standard UI interaction and
/// presentation. Return true from `tick` only when visible state changed.
pub trait Application: super::content::Content {
    fn view(&self) -> View;
    /// Opt into same-session executable updates. KIT checks only while the app
    /// explicitly permits replacement; jobs must remain guarded by the app.
    fn live_updates(&self) -> bool {
        false
    }
    fn reload_ready(&self) -> bool {
        false
    }
    fn prepare_reload(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn reload_failed(&mut self, _message: &str) {}
    fn commands(&self) -> Vec<Command>;
    fn event(&mut self, event: Event);
    /// The application's requested control; hidden controls retain their request.
    fn focus_request(&self) -> Option<super::Id> {
        None
    }
    /// Physical base size for shared controls; STAR/FLUX uses 12 px body text.
    fn native_font_size(&self) -> u16 {
        12
    }
    fn native_pane_border_pixels(&self) -> u16 {
        2
    }
    fn native_flat(&self) -> bool {
        false
    }
    /// Use the display-side terminal family, matching STAR/FLUX.
    fn native_monospace(&self) -> bool {
        true
    }
    /// Stateful domain chords can consume a key before standard editing/navigation.
    /// Releases are filtered by the host. Ordinary shortcuts belong in commands.
    fn intercept_key(&mut self, _key: KeyEvent) -> bool {
        false
    }
    /// Intercept domain drags in the resolved frame's coordinate system.
    /// Return false to keep KIT's normal focus, capture and controls.
    fn intercept_mouse(&mut self, _event: MouseEvent) -> bool {
        false
    }
    /// Consume domain paste input (for example dropped folder URLs) before
    /// ordinary focused-field editing. Return false to retain normal paste.
    fn intercept_paste(&mut self, _text: &str) -> bool {
        false
    }
    /// Inspect resolved geometry for domain paging/custom content.
    /// This callback must not rebuild the UI or start another layout solver.
    fn layout(&mut self, _frame: &Frame) {}
    fn theme(&self) -> Option<Theme> {
        None
    }
    fn tick(&mut self) -> bool {
        false
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn capabilities(&mut self, _capabilities: Capabilities) {}
    fn presentation(&mut self, _cells: bool) {}
    fn output_pending(&mut self, _pending: bool) {}
    fn cancel_pointer(&mut self) {}
    /// Extension events from the selected crossterm build (for example FOLD's
    /// OSC drop notifications) retain their domain callback without forking UI.
    fn terminal_event(&mut self, _event: crate::crossterm::event::Event) -> bool {
        false
    }
    fn attached(&mut self) {}
    fn detached(&mut self) {}
    fn closed(&self) -> bool;
    fn shutdown(&mut self) {}
    /// Domain input for embedded views, dropped files and frontend notices.
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn input(&mut self, _input: Input) -> bool {
        false
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn media(&mut self, _message: crate::terminal_graphics::media::ToHost) -> bool {
        false
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn frontend_action(
        &mut self,
        _message: crate::terminal_graphics::frontend_actions::ToHost,
    ) -> bool {
        false
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn media_chunks(&mut self) -> Vec<ServerMessage> {
        vec![]
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn live_frames(&mut self) -> Vec<crate::terminal_graphics::live_canvas::Update> {
        vec![]
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn effects(&mut self) -> Vec<ServerMessage> {
        vec![]
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    fn export(&mut self, _message: crate::terminal_graphics::exports::ToHost) -> bool {
        false
    }
}

/// Controller adapter retains the last scene and never rebuilds a quiet app.
pub struct Host<A: Application> {
    app: A,
    ui: Runtime,
    #[cfg(all(feature = "terminal-graphics", unix))]
    cells: bool,
    #[cfg(all(feature = "terminal-graphics", unix))]
    scene: Option<Scene>,
    #[cfg(all(feature = "terminal-graphics", unix))]
    presentation_requested: bool,
    repaint_requested: bool,
    failure: Option<String>,
    #[cfg(unix)]
    reloader: Option<super::reload::Probe>,
}
impl<A: Application> Host<A> {
    pub fn new(app: A, theme: Theme) -> anyhow::Result<Self> {
        let mut ui = Runtime::new(app.view(), Registry::new(app.commands())?, theme)?;
        ui.native_monospace(app.native_monospace());
        ui.native_flat(app.native_flat());
        ui.native_pane_border_pixels(app.native_pane_border_pixels());
        ui.native_font_size(app.native_font_size());
        if let Some(id) = app.focus_request() {
            ui.request_focus(id)?;
        }
        #[cfg(unix)]
        let reloader = if app.live_updates() {
            super::reload::Probe::new()
        } else {
            None
        };
        Ok(Self {
            #[cfg(unix)]
            reloader,
            app,
            ui,
            #[cfg(all(feature = "terminal-graphics", unix))]
            cells: false,
            #[cfg(all(feature = "terminal-graphics", unix))]
            scene: None,
            #[cfg(all(feature = "terminal-graphics", unix))]
            presentation_requested: false,
            repaint_requested: false,
            failure: None,
        })
    }
    pub fn runtime(&self) -> &Runtime {
        &self.ui
    }
    pub fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self.ui
    }
    pub fn app(&self) -> &A {
        &self.app
    }
    pub fn frame(&self) -> Option<&Frame> {
        self.ui.current_frame()
    }
    pub fn error(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    fn refresh(&mut self) {
        self.ui.native_monospace(self.app.native_monospace());
        self.ui.native_flat(self.app.native_flat());
        self.ui
            .native_pane_border_pixels(self.app.native_pane_border_pixels());
        self.ui.native_font_size(self.app.native_font_size());
        if let Some(theme) = self.app.theme() {
            if self.ui.theme() != &theme {
                self.ui.set_theme(theme);
            }
        }
        let captured = self.ui.captured().is_some();
        let result = Registry::new(self.app.commands()).and_then(|commands| {
            self.ui.update(self.app.view(), commands)?;
            if let Some(id) = self.app.focus_request() {
                self.ui.request_focus(id)?;
            }
            Ok(())
        });
        if captured && self.ui.captured().is_none() {
            self.app.cancel_pointer();
        }
        if let Err(error) = result {
            tracing::error!(%error, "Application UI update failed");
            self.failure = Some(format!("{error:#}"));
        }
    }
    fn dispatch(&mut self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        for event in events {
            if let Event::Command(id) = &event {
                if id.0 == "kit.repaint" {
                    self.repaint_requested = true;
                    self.ui.invalidate();
                    continue;
                }
                #[cfg(all(feature = "terminal-graphics", unix))]
                if id.0 == "kit.presentation" {
                    self.presentation_requested = true;
                    continue;
                }
            }
            self.app.event(event);
        }
        self.refresh();
    }
    fn paste(&mut self, text: &str) -> Vec<Event> {
        if self.app.intercept_paste(text) {
            self.refresh();
            vec![]
        } else {
            self.ui.paste(text)
        }
    }
    fn key(&mut self, key: KeyEvent) -> Vec<Event> {
        if key.kind == KeyEventKind::Release {
            return vec![];
        }
        if self.app.intercept_key(key) {
            self.refresh();
            vec![]
        } else {
            self.ui.key(key)
        }
    }
}
impl<A: Application> Host<A> {
    /// Cell-only applications use the same state/command contract without
    /// linking a rasterizer, codecs or audio devices. Terminal ownership and
    /// repaint recovery stay in KIT.
    pub fn mouse(&mut self, mouse: MouseEvent) -> Vec<Event> {
        if self.app.intercept_mouse(mouse) {
            if matches!(mouse.kind, MouseEventKind::Up(_)) {
                self.ui.cancel_pointer();
            }
            self.refresh();
            return vec![];
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.ui.pointer(mouse.column, mouse.row, true)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.ui.pointer(mouse.column, mouse.row, false)
            }
            MouseEventKind::Drag(MouseButton::Left) => self.ui.drag(mouse.column, mouse.row),
            MouseEventKind::ScrollUp => self.ui.scroll(mouse.column, mouse.row, -3),
            MouseEventKind::ScrollDown => self.ui.scroll(mouse.column, mouse.row, 3),
            _ => vec![],
        }
    }
    pub fn run_cells(mut self) -> anyhow::Result<()> {
        use crate::crossterm::event::{self, Event as InputEvent};
        let profile = crate::presentation::Profile::resolve(
            &crate::presentation::Environment::detect(),
            crate::presentation::Observed::default(),
        );
        let mut terminal = crate::term::Session::new()?;
        self.app.presentation(true);
        self.app.attached();
        self.refresh();
        let mut generation = 1;
        let result = (|| -> anyhow::Result<()> {
            while !self.app.closed() {
                if let Some(error) = &self.failure {
                    anyhow::bail!("{error}");
                }
                if self.app.tick() {
                    self.refresh();
                }
                let (width, height) = crate::crossterm::terminal::size()?;
                let viewport = super::Viewport {
                    generation,
                    ..super::Viewport::cells(width, height)
                };
                if self.repaint_requested {
                    terminal.clear()?;
                    self.ui.invalidate();
                    self.repaint_requested = false;
                }
                if self.ui.needs_frame()
                    || self
                        .ui
                        .current_frame()
                        .is_none_or(|frame| frame.viewport != viewport)
                {
                    let mut buffer = self.ui.render_cells_with(viewport, &mut self.app)?;
                    if let Some(frame) = self.ui.current_frame() {
                        self.app.layout(frame);
                    }
                    profile.adapt_buffer(&mut buffer);
                    terminal.draw(|frame| *frame.buffer_mut() = buffer)?;
                }
                if !event::poll(std::time::Duration::from_millis(33))? {
                    continue;
                }
                let events = match event::read()? {
                    InputEvent::Key(key) => self.key(key),
                    InputEvent::Paste(text) => self.paste(&text),
                    InputEvent::Mouse(mouse) => self.mouse(mouse),
                    InputEvent::Resize(_, _) => {
                        generation += 1;
                        self.ui.cancel_pointer();
                        self.app.cancel_pointer();
                        self.ui.invalidate();
                        vec![]
                    }
                    InputEvent::FocusGained => {
                        self.repaint_requested = true;
                        vec![]
                    }
                    InputEvent::FocusLost => {
                        self.ui.cancel_pointer();
                        self.app.cancel_pointer();
                        vec![]
                    }
                    #[allow(unreachable_patterns)]
                    // consumer builds may add terminal extension events
                    other => {
                        if self.app.terminal_event(other) {
                            self.refresh();
                        }
                        vec![]
                    }
                };
                self.dispatch(events);
            }
            Ok(())
        })();
        self.app.detached();
        self.app.shutdown();
        let restored = terminal.finish();
        result.and(restored)
    }
}
#[cfg(all(feature = "terminal-graphics", unix))]
impl<A: Application> Controller for Host<A> {
    fn tick(&mut self) {
        if self.app.tick() {
            self.refresh();
        }
        if self.app.reload_ready() {
            if let Some(path) = self.reloader.as_mut().and_then(|p| p.replacement()) {
                let error = match self.app.prepare_reload() {
                    Ok(()) => super::reload::replace(&path).to_string(),
                    Err(error) => format!("{error:#}"),
                };
                self.app.reload_failed(&error);
                self.refresh();
            }
        }
    }
    fn scene_dirty(&self) -> bool {
        self.ui.needs_frame()
    }
    fn scene(&mut self, viewport: Viewport) -> Scene {
        if self.ui.captured().is_some()
            && self
                .scene
                .as_ref()
                .is_some_and(|scene| scene.viewport != viewport)
        {
            self.ui.cancel_pointer();
            self.app.cancel_pointer();
        }
        if self.ui.needs_frame() || self.scene.as_ref().is_none_or(|s| s.viewport != viewport) {
            match self
                .ui
                .render_scene_with(viewport, self.cells, &mut self.app)
            {
                Ok(scene) => {
                    if let Some(frame) = self.ui.current_frame() {
                        self.app.layout(frame);
                    }
                    self.scene = Some(scene);
                }
                Err(error) => {
                    tracing::error!(%error, "Application UI scene failed");
                    self.failure = Some(format!("{error:#}"));
                }
            }
        }
        self.scene
            .clone()
            .unwrap_or_else(|| crate::native_surface::style::Style::default().scene(viewport))
    }
    fn input(&mut self, input: Input) {
        let events = match input {
            Input::Key {
                ref code,
                modifiers,
            } => decode_key(code, modifiers)
                .map(|key| self.key(key))
                .unwrap_or_default(),
            Input::Paste { ref text } => self.paste(text),
            Input::Pointer {
                ref action,
                button,
                x,
                y,
                pixel,
                modifiers,
            } if matches!(
                action.as_str(),
                "down" | "up" | "drag" | "scroll_up" | "scroll_down"
            ) && button <= 2 =>
            {
                let button = match button {
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    _ => MouseButton::Left,
                };
                let (x, y) = if self.cells {
                    (x, y)
                } else if let Some([x, y]) = pixel {
                    (
                        x.min(u32::from(u16::MAX)) as u16,
                        y.min(u32::from(u16::MAX)) as u16,
                    )
                } else if let Some(scene) = &self.scene {
                    (
                        ((u32::from(x) * scene.viewport.width) / u32::from(scene.viewport.columns))
                            as u16,
                        ((u32::from(y) * scene.viewport.height) / u32::from(scene.viewport.rows))
                            as u16,
                    )
                } else {
                    return;
                };
                let kind = match action.as_str() {
                    "drag" => MouseEventKind::Drag(button),
                    "scroll_up" => MouseEventKind::ScrollUp,
                    "scroll_down" => MouseEventKind::ScrollDown,
                    "down" => MouseEventKind::Down(button),
                    _ => MouseEventKind::Up(button),
                };
                self.mouse(MouseEvent {
                    kind,
                    column: x,
                    row: y,
                    modifiers: crate::crossterm::event::KeyModifiers::from_bits_truncate(modifiers),
                })
            }
            Input::CancelPointer => {
                self.ui.cancel_pointer();
                self.app.cancel_pointer();
                vec![]
            }
            _ => {
                if self.app.input(input) {
                    self.refresh();
                }
                vec![]
            }
        };
        self.dispatch(events);
    }
    fn capabilities(&mut self, capabilities: Capabilities) {
        self.cells = capabilities.cell_presentation.unwrap_or(
            capabilities.image_transport
                == crate::terminal_graphics::capabilities::ImageTransport::None
                || !capabilities.native_surfaces,
        );
        self.app.presentation(self.cells);
        self.ui.native_skins(capabilities.native_skins);
        self.app.capabilities(capabilities);
        self.refresh();
    }
    fn supports_presentation_switch(&self) -> bool {
        true
    }
    fn presentation(&mut self, cells: bool) {
        self.cells = cells;
        self.app.presentation(cells);
        self.refresh();
        self.ui.cancel_pointer();
        self.app.cancel_pointer();
        self.ui.invalidate();
    }
    fn attached(&mut self) {
        self.app.attached();
        self.ui.invalidate();
    }
    fn detached(&mut self) {
        self.ui.cancel_pointer();
        self.app.cancel_pointer();
        self.app.detached();
    }
    fn media(&mut self, message: crate::terminal_graphics::media::ToHost) {
        if self.app.media(message) {
            self.refresh();
        }
    }
    fn frontend_action(&mut self, message: crate::terminal_graphics::frontend_actions::ToHost) {
        if self.app.frontend_action(message) {
            self.refresh();
        }
    }
    fn media_chunks(&mut self) -> Vec<ServerMessage> {
        self.app.media_chunks()
    }
    fn live_frames(&mut self) -> Vec<crate::terminal_graphics::live_canvas::Update> {
        self.app.live_frames()
    }
    fn effects(&mut self) -> Vec<ServerMessage> {
        let mut effects = self.app.effects();
        if std::mem::take(&mut self.presentation_requested) {
            effects.push(ServerMessage::TogglePresentation);
        }
        effects
    }
    fn output_pending(&mut self, pending: bool) {
        self.app.output_pending(pending);
    }
    fn export(&mut self, message: crate::terminal_graphics::exports::ToHost) {
        if self.app.export(message) {
            self.refresh();
        }
    }
    fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    fn closed(&self) -> bool {
        self.failure.is_some() || self.app.closed()
    }
    fn shutdown(&mut self) {
        self.app.shutdown();
    }
}
#[cfg(all(feature = "terminal-graphics", unix))]
fn decode_key(code: &str, modifiers: u8) -> Option<KeyEvent> {
    let key = if let Some(character) = code.strip_prefix("char:") {
        let mut chars = character.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        KeyCode::Char(c)
    } else if let Some(number) = code.strip_prefix("f:") {
        KeyCode::F(number.parse().ok()?)
    } else {
        match code {
            "enter" => KeyCode::Enter,
            "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            _ => return None,
        }
    };
    Some(KeyEvent::new(
        key,
        KeyModifiers::from_bits_truncate(modifiers),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crossterm::event::{KeyCode, KeyModifiers},
        theme::{ThemeFile, BUILTINS},
        ui::{Id, Node, Viewport as UiViewport, Widget},
    };
    struct Example {
        focus: Id,
        value: String,
        interceptions: usize,
        layout_width: u16,
        wheel: i16,
    }
    impl super::super::content::Content for Example {}
    impl Application for Example {
        fn view(&self) -> View {
            View::new(Node::column(
                "root",
                vec![
                    Node::field("field", &self.value),
                    Node::new(
                        "article",
                        Widget::Custom {
                            fallback: "Article".into(),
                            revision: 0,
                        },
                    ),
                ],
            ))
        }
        fn commands(&self) -> Vec<Command> {
            vec![]
        }
        fn event(&mut self, event: Event) {
            match event {
                Event::Focused(id) => self.focus = id,
                Event::CustomScroll { lines, .. } => self.wheel += lines,
                _ => {}
            }
        }
        fn focus_request(&self) -> Option<Id> {
            Some(self.focus.clone())
        }
        fn intercept_key(&mut self, key: KeyEvent) -> bool {
            if key.code == KeyCode::F(2) {
                self.interceptions += 1;
                self.value = "Chord".into();
                true
            } else {
                false
            }
        }
        fn intercept_mouse(&mut self, mouse: MouseEvent) -> bool {
            if mouse.modifiers.contains(KeyModifiers::ALT)
                || mouse.kind == MouseEventKind::Down(MouseButton::Right)
            {
                self.interceptions += 1;
                true
            } else {
                false
            }
        }
        fn intercept_paste(&mut self, text: &str) -> bool {
            if text.starts_with("file://") {
                self.interceptions += 1;
                self.value = "Dropped folder".into();
                true
            } else {
                false
            }
        }
        fn layout(&mut self, frame: &Frame) {
            self.layout_width = frame.viewport.width;
        }
        fn closed(&self) -> bool {
            false
        }
    }
    fn host() -> Host<Example> {
        Host::new(
            Example {
                focus: "field".into(),
                value: String::new(),
                interceptions: 0,
                layout_width: 0,
                wheel: 0,
            },
            Theme::resolve(&ThemeFile::parse(BUILTINS[0].toml).unwrap()),
        )
        .unwrap()
    }
    #[test]
    fn host_delivers_focus_changes_and_consumed_chords_without_replaying_key_releases() {
        let mut host = host();
        host.ui.frame(UiViewport::cells(80, 24)).unwrap();
        let events = host.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        host.dispatch(events);
        assert_eq!(host.app.focus, Id::from("article"));
        assert_eq!(
            host.ui.frame(UiViewport::cells(80, 24)).unwrap().focus,
            Some("article".into())
        );
        let mut release = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(host.key(release).is_empty());
        assert_eq!(host.app.interceptions, 0);
        assert!(host
            .key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE))
            .is_empty());
        assert_eq!(host.app.interceptions, 1);
        assert!(
            matches!(&host.ui.view().root.children[0].widget,Widget::Field { value,.. } if value=="Chord")
        );
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    #[test]
    fn native_right_click_reaches_application_without_activating_left_controls() {
        let mut host = host();
        host.ui.render_cells(UiViewport::cells(80, 24)).unwrap();
        <Host<Example> as Controller>::input(
            &mut host,
            Input::Pointer {
                action: "down".into(),
                button: 1,
                modifiers: 0,
                x: 1,
                y: 0,
                pixel: Some([1, 0]),
            },
        );
        assert_eq!(host.app.interceptions, 1);
        assert!(host.ui.captured().is_none());
    }
    #[test]
    fn consumed_mouse_input_cancels_release_capture_without_replaying_it() {
        let mut host = host();
        host.ui.render_cells(UiViewport::cells(80, 24)).unwrap();
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let events = host.mouse(mouse);
        host.dispatch(events);
        assert!(host.ui.captured().is_some());
        assert!(host
            .mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                modifiers: KeyModifiers::ALT,
                ..mouse
            })
            .is_empty());
        assert!(host
            .mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                modifiers: KeyModifiers::ALT,
                ..mouse
            })
            .is_empty());
        assert_eq!(host.app.interceptions, 2);
        assert!(host.ui.captured().is_none());
    }
    #[test]
    fn domain_paste_precedes_field_editing_and_ordinary_paste_still_edits() {
        let mut host = host();
        host.ui.render_cells(UiViewport::cells(80, 24)).unwrap();
        host.ui.request_focus("field".into()).unwrap();
        assert!(
            matches!(&host.paste("hello")[..], [Event::Changed {value,..}] if value == "hello")
        );
        assert!(host.paste("file:///folder").is_empty());
        assert_eq!(host.app.interceptions, 1);
        assert!(
            matches!(&host.ui.view().root.children[0].widget, Widget::Field {value,..} if value == "Dropped folder")
        );
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    #[test]
    fn graphical_paste_uses_the_same_domain_interception() {
        let mut host = host();
        host.input(Input::Paste {
            text: "file:///folder".into(),
        });
        assert_eq!(host.app.interceptions, 1);
    }
    #[cfg(all(feature = "terminal-graphics", unix))]
    #[test]
    fn native_host_reports_display_geometry_and_routes_bounded_wheel_input() {
        let mut host = host();
        let viewport = Viewport {
            width: 800,
            height: 600,
            ..Default::default()
        };
        host.scene(viewport);
        assert_eq!(host.app.layout_width, 800);
        let area = host.frame().unwrap().get(&"article".into()).unwrap().body;
        host.input(Input::Pointer {
            action: "scroll_down".into(),
            button: 0,
            x: 0,
            y: 0,
            pixel: Some([u32::from(area.x + 1), u32::from(area.y + 1)]),
            modifiers: 0,
        });
        assert_eq!(host.app.wheel, 3);
        host.scene(Viewport {
            width: 400,
            ..viewport
        });
        assert_eq!(host.app.layout_width, 400);
    }
}
