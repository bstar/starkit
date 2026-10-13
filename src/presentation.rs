//! Explicit presentation policy, separate from probes and application state.
//! Environment hints never establish graphics, pixel input or speaker access.
use crate::ratatui::{buffer::Buffer, style::Color};
use serde::{Deserialize, Serialize};
use std::io::IsTerminal as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorDepth {
    #[default]
    Mono,
    Ansi16,
    Indexed256,
    TrueColor,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Glyphs {
    #[default]
    Ascii,
    Unicode,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Images {
    #[default]
    None,
    HalfBlocks,
    Kitty,
    Sixel,
    Iterm2,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Ascii,
    Cells,
    Native,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    #[default]
    Local,
    SshShell,
    Bridge,
}
/// Only sanitized hints used by policy. No terminal queries or raw-mode input.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub interactive: bool,
    pub term: String,
    pub colorterm: String,
    pub utf8: bool,
    pub no_color: bool,
    pub ascii: bool,
    pub ssh: bool,
    pub multiplexer: bool,
}
impl Environment {
    pub fn detect() -> Self {
        let var = |key| std::env::var(key).unwrap_or_default();
        let locale = [var("LC_ALL"), var("LC_CTYPE"), var("LANG")]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .to_ascii_lowercase();
        Self {
            interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            term: var("TERM"),
            colorterm: var("COLORTERM"),
            utf8: locale.contains("utf-8") || locale.contains("utf8"),
            no_color: std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
            ascii: var("STAR_KIT_ASCII") == "1",
            ssh: std::env::var_os("SSH_CONNECTION").is_some()
                || std::env::var_os("SSH_TTY").is_some(),
            multiplexer: std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some(),
        }
    }
}
/// Supply actual probe/negotiation results. Defaults promise no optional feature.
/// Audio denotes an available frontend implementation, not an opened device;
/// device-open errors must still be reported without switching playback hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observed {
    pub images: Images,
    pub native_renderer: bool,
    pub measured_pixels: bool,
    pub pixel_mouse: bool,
    pub audio_frontend: bool,
    pub video_decoder: bool,
    pub trusted_paths: bool,
    pub bridge: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub color: ColorDepth,
    pub ascii_requested: bool,
    pub glyphs: Glyphs,
    pub images: Images,
    pub native: bool,
    pub pixel_mouse: bool,
    pub keyboard: bool,
    pub audio: bool,
    pub video: bool,
    pub local_paths: bool,
    pub connection: Connection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    NonInteractive,
    AsciiRequested,
    UnicodeUnavailable,
    GraphicsUnavailable,
    NativeRendererUnavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub requested: Tier,
    pub selected: Tier,
    pub reasons: Vec<Fallback>,
}
impl Profile {
    pub fn resolve(env: &Environment, observed: Observed) -> Self {
        let dumb = env.term.is_empty() || env.term == "dumb";
        let glyphs = if env.ascii || !env.utf8 || dumb {
            Glyphs::Ascii
        } else {
            Glyphs::Unicode
        };
        let color = if env.no_color || dumb {
            ColorDepth::Mono
        } else if matches!(
            env.colorterm.to_ascii_lowercase().as_str(),
            "truecolor" | "24bit"
        ) {
            ColorDepth::TrueColor
        } else if env.term.contains("256color") {
            ColorDepth::Indexed256
        } else {
            ColorDepth::Ansi16
        };
        let connection = if observed.bridge {
            Connection::Bridge
        } else if env.ssh {
            Connection::SshShell
        } else {
            Connection::Local
        };
        let images = if !env.interactive || env.ascii {
            Images::None
        } else if observed.images == Images::None && glyphs == Glyphs::Unicode {
            Images::HalfBlocks
        } else {
            observed.images
        };
        Self {
            color,
            ascii_requested: env.ascii,
            glyphs,
            images,
            native: env.interactive
                && !env.ascii
                && observed.native_renderer
                && images == Images::Kitty,
            pixel_mouse: env.interactive
                && !env.multiplexer
                && observed.measured_pixels
                && observed.pixel_mouse,
            keyboard: env.interactive,
            audio: observed.audio_frontend && connection != Connection::SshShell,
            video: observed.video_decoder,
            local_paths: observed.trusted_paths && connection != Connection::SshShell,
            connection,
        }
    }
    pub fn choose(&self, requested: Tier) -> Selection {
        let mut reasons = Vec::new();
        let selected = if requested == Tier::Ascii {
            Tier::Ascii
        } else if !self.keyboard {
            reasons.push(Fallback::NonInteractive);
            Tier::Ascii
        } else if self.glyphs == Glyphs::Ascii {
            reasons.push(if self.ascii_requested {
                Fallback::AsciiRequested
            } else {
                Fallback::UnicodeUnavailable
            });
            Tier::Ascii
        } else if requested == Tier::Native && self.native {
            Tier::Native
        } else {
            if requested == Tier::Native {
                reasons.push(if self.images == Images::Kitty {
                    Fallback::NativeRendererUnavailable
                } else {
                    Fallback::GraphicsUnavailable
                });
            }
            Tier::Cells
        };
        Selection {
            requested,
            selected,
            reasons,
        }
    }
    /// Final cell adaptation for an ASCII or reduced-color terminal. Keep semantic
    /// state and geometry intact; applications should use modifiers for selection
    /// in monochrome themes. This never starts a probe or emits terminal escapes.
    pub fn adapt_buffer(&self, buffer: &mut Buffer) {
        let mut colors = std::collections::HashMap::new();
        let mut map = |color| {
            if let Some(mapped) = colors.get(&color) {
                return *mapped;
            }
            let mapped = self.color.map(color);
            if colors.len() < 256 {
                colors.insert(color, mapped);
            }
            mapped
        };
        for cell in &mut buffer.content {
            cell.fg = map(cell.fg);
            cell.bg = map(cell.bg);
            cell.underline_color = map(cell.underline_color);
            if self.glyphs == Glyphs::Ascii && !cell.symbol().is_ascii() {
                let text: String = cell.symbol().chars().map(ascii).collect();
                cell.set_symbol(&text);
            }
        }
    }
}
fn ascii(c: char) -> char {
    match c {
        '─' | '━' | '═' => '-',
        '│' | '┃' | '║' => '|',
        '┌' | '┐' | '└' | '┘' | '╭' | '╮' | '╰' | '╯' | '┬' | '┴' | '├' | '┤' | '┼' | '╔' | '╗'
        | '╚' | '╝' | '╬' => '+',
        '…' => '.',
        '←' | '◀' | '◁' => '<',
        '→' | '▶' | '▷' => '>',
        '↑' | '▲' => '^',
        '↓' | '▼' => 'v',
        '█' | '▓' | '▒' | '░' | '▀' | '▄' => '#',
        '✓' | '✔' => 'x',
        '•' | '·' | '●' | '○' => '*',
        c if c.is_ascii() && !c.is_control() => c,
        _ => '?',
    }
}
const ANSI: [[u8; 3]; 16] = [
    [0, 0, 0],
    [128, 0, 0],
    [0, 128, 0],
    [128, 128, 0],
    [0, 0, 128],
    [128, 0, 128],
    [0, 128, 128],
    [192, 192, 192],
    [128, 128, 128],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [0, 0, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];
fn indexed(index: u8) -> [u8; 3] {
    if index < 16 {
        ANSI[usize::from(index)]
    } else if index >= 232 {
        [8 + (index - 232) * 10; 3]
    } else {
        let n = index - 16;
        let channel = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
        [channel(n / 36), channel(n / 6 % 6), channel(n % 6)]
    }
}
fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).pow(2) as u32)
        .sum()
}
impl ColorDepth {
    pub fn map(self, color: Color) -> Color {
        if color == Color::Reset {
            return color;
        }
        if self == Self::Mono {
            return Color::Reset;
        }
        let rgb = match color {
            Color::Rgb(r, g, b) => [r, g, b],
            Color::Indexed(i) => indexed(i),
            other => return other,
        };
        if self == Self::TrueColor {
            return color;
        }
        let range = if self == Self::Ansi16 {
            0..16u16
        } else {
            0..256u16
        };
        Color::Indexed(
            range
                .min_by_key(|n| distance(rgb, indexed(*n as u8)))
                .unwrap() as u8,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn env() -> Environment {
        Environment {
            interactive: true,
            term: "xterm-256color".into(),
            utf8: true,
            ..Default::default()
        }
    }
    #[test]
    fn hints_never_invent_graphics_audio_or_pixel_input() {
        let p = Profile::resolve(&env(), Observed::default());
        assert_eq!(p.choose(Tier::Native).selected, Tier::Cells);
        assert!(!p.native && !p.pixel_mouse && !p.audio && !p.video && !p.local_paths);
        assert_eq!(p.images, Images::HalfBlocks);
        let mut e = env();
        e.ascii = true;
        assert_eq!(
            Profile::resolve(&e, Observed::default())
                .choose(Tier::Native)
                .selected,
            Tier::Ascii
        );
    }
    #[test]
    fn ssh_and_multiplexer_keep_independent_fallbacks() {
        let mut e = env();
        e.ssh = true;
        e.multiplexer = true;
        let mut o = Observed {
            images: Images::Kitty,
            native_renderer: true,
            measured_pixels: true,
            pixel_mouse: true,
            audio_frontend: true,
            video_decoder: true,
            trusted_paths: true,
            bridge: false,
        };
        let p = Profile::resolve(&e, o);
        assert!(p.native && p.video && !p.pixel_mouse && !p.audio && !p.local_paths);
        o.bridge = true;
        let p = Profile::resolve(&e, o);
        assert!(p.audio && p.local_paths);
    }
    #[test]
    fn ascii_color_adaptation_preserves_geometry_and_attributes() {
        use crate::ratatui::{layout::Rect, style::Modifier};
        let mut b = Buffer::empty(Rect::new(0, 0, 3, 1));
        b[(0, 0)]
            .set_symbol("┌")
            .set_fg(Color::Rgb(255, 1, 1))
            .set_bg(Color::Indexed(20));
        b[(0, 0)].modifier = Modifier::REVERSED;
        let mut e = env();
        e.ascii = true;
        let p = Profile::resolve(&e, Observed::default());
        p.adapt_buffer(&mut b);
        assert_eq!(b[(0, 0)].symbol(), "+");
        assert_eq!(b[(0, 0)].fg, Color::Indexed(9));
        assert!(b[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(
            ColorDepth::Ansi16.map(Color::Indexed(196)),
            Color::Indexed(9)
        );
        assert_eq!(ColorDepth::Mono.map(Color::Red), Color::Reset);
    }
}
