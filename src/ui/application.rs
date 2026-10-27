//! Adapter from the application UI contract to KIT's persistent local/SSH host.
use super::{
    command::{Command, Registry},
    Event, Frame, Runtime, View,
};
use crate::theme::Theme;
#[cfg(all(feature = "terminal-graphics", unix))]
use crate::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
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
    fn commands(&self) -> Vec<Command>;
    fn event(&mut self, event: Event);
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
}
impl<A: Application> Host<A> {
    pub fn new(app: A, theme: Theme) -> anyhow::Result<Self> {
        let ui = Runtime::new(app.view(), Registry::new(app.commands())?, theme)?;
        Ok(Self {
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
        if let Some(theme) = self.app.theme() {
            if self.ui.theme() != &theme {
                self.ui.set_theme(theme);
            }
        }
        let captured = self.ui.captured().is_some();
        let result = Registry::new(self.app.commands()).and_then(|commands| {
            self.ui.update(self.app.view(), commands)?;
            Ok(())
        });
        if captured && self.ui.captured().is_none() {
            self.app.cancel_pointer();
        }
        if let Err(error) = result {
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
}
impl<A: Application> Host<A> {
    /// Cell-only applications use the same state/command contract without
    /// linking a rasterizer, codecs or audio devices. Terminal ownership and
    /// repaint recovery stay in KIT.
    pub fn run_cells(mut self) -> anyhow::Result<()> {
        use crate::crossterm::event::{self, Event as InputEvent, MouseButton, MouseEventKind};
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
                    profile.adapt_buffer(&mut buffer);
                    terminal.draw(|frame| *frame.buffer_mut() = buffer)?;
                }
                if !event::poll(std::time::Duration::from_millis(33))? {
                    continue;
                }
                let events = match event::read()? {
                    InputEvent::Key(key) => self.ui.key(key),
                    InputEvent::Paste(text) => self.ui.paste(&text),
                    InputEvent::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            self.ui.pointer(mouse.column, mouse.row, true)
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            self.ui.pointer(mouse.column, mouse.row, false)
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            self.ui.drag(mouse.column, mouse.row)
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            self.ui.scroll(
                                mouse.column,
                                mouse.row,
                                if mouse.kind == MouseEventKind::ScrollUp {
                                    -3
                                } else {
                                    3
                                },
                            );
                            vec![]
                        }
                        _ => vec![],
                    },
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
                Ok(scene) => self.scene = Some(scene),
                Err(error) => self.failure = Some(format!("{error:#}")),
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
                .map(|key| self.ui.key(key))
                .unwrap_or_default(),
            Input::Paste { ref text } => self.ui.paste(text),
            Input::Pointer {
                ref action,
                button: 0,
                x,
                y,
                pixel,
                ..
            } if matches!(
                action.as_str(),
                "down" | "up" | "drag" | "scroll_up" | "scroll_down"
            ) =>
            {
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
                match action.as_str() {
                    "drag" => self.ui.drag(x, y),
                    "scroll_up" | "scroll_down" => {
                        self.ui
                            .scroll(x, y, if action == "scroll_up" { -3 } else { 3 });
                        vec![]
                    }
                    _ => self.ui.pointer(x, y, action == "down"),
                }
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
