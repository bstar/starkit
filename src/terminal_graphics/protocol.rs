//! Versioned bounded messages shared by application hosts and local renderers.
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub const MAX_CELLS: usize = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub columns: u16,
    pub rows: u16,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            columns: 100,
            rows: 40,
            width: 1200,
            height: 800,
            generation: 1,
        }
    }
}
impl Viewport {
    pub fn validate(self) -> io::Result<Self> {
        if self.columns == 0
            || self.rows == 0
            || self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || u64::from(self.width) * u64::from(self.height) > 32_000_000
            || usize::from(self.columns) * usize::from(self.rows) > MAX_CELLS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "graphical viewport exceeds limits",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
impl Rect {
    pub fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && u32::from(x) < u32::from(self.x) + u32::from(self.width)
            && u32::from(y) < u32::from(self.y) + u32::from(self.height)
    }
}
impl From<crate::ratatui::layout::Rect> for Rect {
    fn from(r: crate::ratatui::layout::Rect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub x: u16,
    pub y: u16,
    pub text: String,
    pub foreground: String,
    pub background: String,
    pub bold: bool,
}

/// Graphical primitives have the same coordinate space as controller hit tests.
/// Lists contain only the visible range; full labels do not become multiline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Component {
    Surface {
        rect: Rect,
        surface: super::surface::Surface,
    },
    Menu {
        rect: Rect,
    },
    Dialog {
        rect: Rect,
        title: String,
    },
    TextField {
        rect: Rect,
        text: String,
        caret: usize,
        secret: bool,
    },
    Panel {
        rect: Rect,
        active: bool,
    },
    ListRow {
        rect: Rect,
        label: String,
        icon: String,
        foreground: String,
        background: String,
        selected: bool,
        marked: bool,
    },
    Tab {
        rect: Rect,
        label: String,
        active: bool,
        #[serde(default)]
        close: Option<Rect>,
    },
    Meter {
        rect: Rect,
        value: u16,
        foreground: String,
        background: String,
    },
    /// Continuous pixel scrollbar over the controller's existing cell hit track.
    Scrollbar {
        rect: Rect,
        thumb: Rect,
    },
    Image {
        rect: Rect,
        id: String,
        png: Option<String>,
    },
    Terminal {
        rect: Rect,
    },
}

impl Component {
    pub fn rect(&self) -> Rect {
        match self {
            Self::Surface { rect, .. }
            | Self::Menu { rect }
            | Self::Dialog { rect, .. }
            | Self::TextField { rect, .. }
            | Self::Panel { rect, .. }
            | Self::ListRow { rect, .. }
            | Self::Tab { rect, .. }
            | Self::Meter { rect, .. }
            | Self::Scrollbar { rect, .. }
            | Self::Image { rect, .. }
            | Self::Terminal { rect } => *rect,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scene {
    pub revision: u64,
    /// Changes when hit targets change; ordinary progress/selection paint does not.
    #[serde(default)]
    pub interaction: u64,
    /// Stable while wheel destinations are unchanged, even when rows move.
    /// None keeps strict hit-target admission (including legacy controllers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_interaction: Option<u64>,
    pub viewport: Viewport,
    pub background: String,
    pub foreground: String,
    #[serde(default)]
    pub accent: String,
    #[serde(default)]
    pub border: String,
    pub spans: Vec<Span>,
    pub components: Vec<Component>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placements: Vec<super::placement::Placement>,
}
impl Scene {
    pub fn from_buffer(
        buffer: &crate::ratatui::buffer::Buffer,
        viewport: Viewport,
        revision: u64,
    ) -> Self {
        use crate::ratatui::style::{Color, Modifier};
        fn color(c: Color, fallback: &str) -> String {
            match c {
                Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
                Color::Black => "#000000".into(),
                Color::White => "#ffffff".into(),
                Color::Red | Color::LightRed => "#f38ba8".into(),
                Color::Green | Color::LightGreen => "#a6e3a1".into(),
                Color::Yellow | Color::LightYellow => "#f9e2af".into(),
                Color::Blue | Color::LightBlue => "#89b4fa".into(),
                Color::Magenta | Color::LightMagenta => "#cba6f7".into(),
                Color::Cyan | Color::LightCyan => "#94e2d5".into(),
                Color::Gray => "#bac2de".into(),
                Color::DarkGray => "#45475a".into(),
                _ => fallback.into(),
            }
        }
        let background = buffer
            .content
            .first()
            .map(|c| color(c.bg, "#1e1e2e"))
            .unwrap_or("#1e1e2e".into());
        let foreground = buffer
            .content
            .first()
            .map(|c| color(c.fg, "#cdd6f4"))
            .unwrap_or("#cdd6f4".into());
        let mut spans: Vec<Span> = Vec::new();
        for y in 0..viewport.rows {
            let mut covered = 0;
            for x in 0..viewport.columns {
                if x < covered {
                    continue;
                }
                let cell = &buffer[(x, y)];
                covered = x.saturating_add(
                    unicode_width::UnicodeWidthStr::width(cell.symbol()).max(1) as u16,
                );
                let reversed = cell.modifier.contains(Modifier::REVERSED);
                let fg = color(if reversed { cell.bg } else { cell.fg }, &foreground);
                let bg = color(if reversed { cell.fg } else { cell.bg }, &background);
                let bold = cell.modifier.contains(Modifier::BOLD);
                // Wide glyphs own their following cells; blank continuation cells
                // must not introduce extra spacing into a coalesced text span.
                let text = cell.symbol().to_string();
                if let Some(last) = spans.last_mut().filter(|s| {
                    s.y == y
                        && s.foreground == fg
                        && s.background == bg
                        && s.bold == bold
                        && usize::from(s.x) + unicode_width::UnicodeWidthStr::width(s.text.as_str())
                            == usize::from(x)
                }) {
                    last.text.push_str(&text);
                } else {
                    spans.push(Span {
                        x,
                        y,
                        text,
                        foreground: fg,
                        background: bg,
                        bold,
                    });
                }
            }
        }
        Self {
            revision,
            interaction: 0,
            scroll_interaction: None,
            viewport,
            background,
            accent: foreground.clone(),
            border: foreground.clone(),
            foreground,
            spans,
            components: vec![],
            placements: vec![],
        }
    }
    pub fn same_content(&self, other: &Self) -> bool {
        self.interaction == other.interaction
            && self.scroll_interaction == other.scroll_interaction
            && self.viewport == other.viewport
            && self.background == other.background
            && self.foreground == other.foreground
            && self.accent == other.accent
            && self.border == other.border
            && self.spans == other.spans
            && self.components == other.components
            && self.placements == other.placements
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Key {
        code: String,
        modifiers: u8,
    },
    Pointer {
        action: String,
        button: u8,
        x: u16,
        y: u16,
        modifiers: u8,
    },
    Paste {
        text: String,
    },
    Osc72 {
        text: String,
    },
    Resize {
        viewport: Viewport,
    },
    Detach,
    CancelPointer,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        version: u16,
        viewport: Viewport,
        client: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<super::capabilities::Capabilities>,
    },
    Input {
        id: u64,
        revision: u64,
        generation: u64,
        input: Input,
    },
    Presented {
        revision: u64,
        generation: u64,
    },
    EffectAck {
        id: u64,
    },
    Ping,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello {
        version: u16,
        session: String,
        epoch: String,
    },
    Scene {
        scene: Scene,
    },
    Ack {
        id: u64,
        accepted: bool,
    },
    Clipboard {
        text: String,
    },
    Asset {
        id: String,
        png: String,
    },
    Osc72 {
        #[serde(default)]
        id: u64,
        meta: String,
        payload: Option<String>,
    },
    Error {
        message: String,
    },
    Closed,
    Pong,
}

pub fn read_message<T: serde::de::DeserializeOwned>(
    input: &mut impl BufRead,
) -> io::Result<Option<T>> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete graphical message",
            ));
        }
        let end = available.iter().position(|b| *b == b'\n');
        let n = end.map_or(available.len(), |i| i + 1);
        if bytes.len().saturating_add(n) > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "graphical message exceeds limit",
            ));
        }
        bytes.extend_from_slice(&available[..n]);
        input.consume(n);
        if end.is_some() {
            break;
        }
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
pub fn write_message(value: &impl Serialize, out: &mut impl Write) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "graphical message exceeds limit",
        ));
    }
    out.write_all(&bytes)?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Monotonic command IDs are scoped to a client and an application epoch.
/// A restarted server has a new epoch; clients never replay its old commands.
#[derive(Default)]
pub struct Admission {
    last: u64,
    presented: Option<(u64, u64, u64)>,
    press: Option<u64>,
    scroll: Option<(u64, u64, u64)>,
}
impl Admission {
    pub fn presented(&mut self, revision: u64, generation: u64) {
        self.presented = Some((revision, generation, 0));
    }
    pub fn target(&mut self, revision: u64, generation: u64, interaction: u64) {
        self.presented = Some((revision, generation, interaction));
    }
    pub fn scroll_target(&mut self, revision: u64, generation: u64, context: Option<u64>) {
        self.scroll = context.map(|context| (revision, generation, context));
    }
    pub fn matches(&self, revision: u64, generation: u64, current: &Scene) -> bool {
        self.presented == Some((revision, generation, current.interaction))
            && generation == current.viewport.generation
    }
    /// A captured gesture owns its release even when rows were repainted.
    /// Resize still invalidates its coordinate system and must cancel it.
    pub(super) fn cancel_release(&self, revision: u64, generation: u64, current: &Scene) -> bool {
        if self.press == Some(generation) && generation == current.viewport.generation {
            false
        } else {
            !self.matches(revision, generation, current)
        }
    }
    pub fn admit(
        &mut self,
        id: u64,
        revision: u64,
        generation: u64,
        current: &Scene,
        input: &Input,
    ) -> bool {
        if id <= self.last {
            return false;
        }
        self.last = id;
        match input {
            Input::Pointer { action, .. } => {
                // Always balance a captured release, even after invalidation.
                if action == "up" && self.press.take().is_some() {
                    return true;
                }
                let current_geometry = generation == current.viewport.generation;
                let fresh = self.matches(revision, generation, current);
                if action == "drag" && self.press == Some(generation) {
                    return current_geometry;
                }
                let wheel = matches!(
                    action.as_str(),
                    "scroll_up" | "scroll_down" | "scroll_left" | "scroll_right"
                );
                let stable_scroll = wheel
                    && current.scroll_interaction.is_some_and(|context| {
                        self.scroll == Some((revision, generation, context))
                    });
                if !current_geometry || !(fresh || stable_scroll) {
                    return false;
                }
                if action == "down" {
                    self.press = Some(generation);
                }
                true
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_burst_survives_row_changes_but_not_destination_changes() {
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                0, 0, 100, 40,
            )),
            Viewport::default(),
            1,
        );
        scene.scroll_interaction = Some(42);
        let mut admission = Admission::default();
        admission.target(1, 1, scene.interaction);
        admission.scroll_target(1, 1, scene.scroll_interaction);
        let pointer = |action: &str| Input::Pointer {
            action: action.into(),
            button: 0,
            x: 5,
            y: 8,
            modifiers: 0,
        };
        // Deliberately withhold presentation: a slow renderer/SSH connection
        // must not turn continuous wheel movement into dropped steps.
        for id in 1..=120 {
            scene.interaction += 1;
            scene.revision += 1;
            assert!(admission.admit(id, 1, 1, &scene, &pointer("scroll_down")));
        }
        assert!(!admission.admit(120, 1, 1, &scene, &pointer("scroll_down")));
        assert!(!admission.admit(121, 1, 1, &scene, &pointer("down")));
        assert!(!admission.admit(122, 1, 2, &scene, &pointer("scroll_down")));
        scene.scroll_interaction = Some(43);
        assert!(!admission.admit(123, 1, 1, &scene, &pointer("scroll_down")));
        scene.scroll_interaction = None;
        assert!(!admission.admit(124, 1, 1, &scene, &pointer("scroll_down")));
        let mut wire = serde_json::to_value(&scene).unwrap();
        wire.as_object_mut().unwrap().remove("scroll_interaction");
        assert_eq!(
            serde_json::from_value::<Scene>(wire)
                .unwrap()
                .scroll_interaction,
            None
        );
    }

    #[test]
    fn captured_drop_survives_repaint_but_resize_and_uncaptured_release_cancel() {
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                0, 0, 100, 40,
            )),
            Viewport::default(),
            1,
        );
        let pointer = |action: &str| Input::Pointer {
            action: action.into(),
            button: 0,
            x: 4,
            y: 8,
            modifiers: 0,
        };
        let mut admission = Admission::default();
        admission.target(1, 1, scene.interaction);
        assert!(admission.admit(1, 1, 1, &scene, &pointer("down")));
        scene.revision = 2;
        scene.interaction += 1;
        assert!(!admission.matches(1, 1, &scene));
        assert!(!admission.cancel_release(1, 1, &scene));
        assert!(admission.admit(2, 1, 1, &scene, &pointer("drag")));
        assert!(admission.admit(3, 1, 1, &scene, &pointer("up")));
        assert!(admission.cancel_release(1, 1, &scene));
        admission.target(2, 1, scene.interaction);
        assert!(admission.admit(4, 2, 1, &scene, &pointer("down")));
        scene.viewport.generation = 2;
        assert!(admission.cancel_release(2, 1, &scene));
        assert!(admission.cancel_release(2, 2, &scene));
    }

    #[test]
    fn bounds_and_truncated_messages() {
        assert!(Viewport {
            width: 8192,
            height: 8192,
            ..Viewport::default()
        }
        .validate()
        .is_err());
        assert!(read_message::<ClientMessage>(&mut &b"{\"type\":\"ping\"}"[..]).is_err());
        assert!(matches!(
            read_message::<ClientMessage>(&mut &b"{\"type\":\"ping\"}\n"[..]).unwrap(),
            Some(ClientMessage::Ping)
        ));
    }
    #[test]
    fn stale_press_rejected_release_balanced_duplicate_rejected() {
        let s = Scene {
            revision: 5,
            interaction: 1,
            scroll_interaction: None,
            viewport: Viewport::default(),
            background: String::new(),
            foreground: String::new(),
            accent: String::new(),
            border: String::new(),
            spans: vec![],
            components: vec![],
            placements: vec![],
        };
        let pointer = |action: &str| Input::Pointer {
            action: action.into(),
            button: 0,
            x: 0,
            y: 0,
            modifiers: 0,
        };
        let mut a = Admission::default();
        a.presented(4, 1);
        assert!(!a.admit(1, 4, 1, &s, &pointer("down")));
        a.target(5, 1, 1);
        assert!(a.admit(2, 5, 1, &s, &pointer("down")));
        assert!(a.admit(3, 5, 2, &s, &pointer("up")));
        assert!(!a.admit(
            3,
            5,
            1,
            &s,
            &Input::Paste {
                text: "duplicate".into()
            }
        ));
    }

    #[test]
    fn progress_repaints_preserve_targets_but_navigation_invalidates_them() {
        let mut scene = Scene {
            revision: 10,
            interaction: 4,
            scroll_interaction: None,
            viewport: Viewport::default(),
            background: String::new(),
            foreground: String::new(),
            accent: String::new(),
            border: String::new(),
            spans: vec![],
            components: vec![],
            placements: vec![],
        };
        let mut admission = Admission::default();
        admission.target(9, 1, 4);
        let press = Input::Pointer {
            action: "down".into(),
            button: 0,
            x: 2,
            y: 3,
            modifiers: 0,
        };
        assert!(admission.admit(1, 9, 1, &scene, &press));
        scene.interaction = 5;
        assert!(!admission.admit(2, 9, 1, &scene, &press));
    }
}
