//! Local terminal typography. Queried before input ownership passes to the UI.
//! Kitty's XTGETTCAP font query reports a PostScript name, not a family name.
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub(crate) struct Font {
    pub name: Option<String>,
    pub pixels: Option<f32>,
    pub cell: Option<(u16, u16)>,
}
impl Font {
    pub fn configured(mut self) -> Self {
        if let Ok(name) = std::env::var("STAR_GRAPHICS_FONT") {
            if !name.trim().is_empty() {
                self.name = Some(name);
            }
        }
        if let Ok(size) = std::env::var("STAR_GRAPHICS_FONT_SIZE") {
            self.pixels = number(&size)
                .filter(|n| (4.0..=96.0).contains(n))
                .or(self.pixels);
        }
        self
    }
}
fn number(s: &str) -> Option<f32> {
    s.parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.)
}
fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Option<String> {
    if s.len() > 1024 || !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    let bytes = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}
fn replies(bytes: &[u8]) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for part in bytes.split(|b| *b == 0x1b) {
        let Ok(s) = std::str::from_utf8(part) else {
            continue;
        };
        let Some(s) = s.strip_prefix("P1+r") else {
            continue;
        };
        for pair in s.split(';') {
            if let Some((key, value)) = pair.split_once('=') {
                if let (Some(key), Some(value)) = (unhex(key), unhex(value)) {
                    values.insert(key, value);
                }
            }
        }
    }
    values
}
fn from_replies(bytes: &[u8]) -> Font {
    let v = replies(bytes);
    let get = |key| v.get(&format!("kitty-query-{key}"));
    let pixels = get("font_size")
        .and_then(|s| number(s))
        .zip(get("dpi_y").and_then(|s| number(s)))
        .map(|(pt, dpi)| pt * dpi / 72.)
        .filter(|n| (4.0..=96.0).contains(n));
    Font {
        name: get("font_family").filter(|s| !s.is_empty()).cloned(),
        pixels,
        cell: None,
    }
}

/// No subprocess, remote-control permission, config-file guesses, or server-side
/// font assumptions. This runs on the presenting client, including SSH sessions.
#[cfg(unix)]
pub(crate) fn probe() -> Font {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::{Duration, Instant};
    let attempt = || -> std::io::Result<Font> {
        let mut tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open("/dev/tty")?;
        let was_raw = is_raw_mode_enabled()?;
        enable_raw_mode()?;
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                if !self.0 {
                    let _ = disable_raw_mode();
                }
            }
        }
        let _restore = Restore(was_raw);
        let keys = ["font_family", "font_size", "dpi_y"];
        let query = keys.map(|k| hex(&format!("kitty-query-{k}"))).join(";");
        write!(tty, "\x1bP+q{query}\x1b\\")?;
        tty.flush()?;
        let deadline = Instant::now() + Duration::from_millis(200);
        let mut bytes = Vec::new();
        while Instant::now() < deadline && bytes.len() < 8192 {
            let mut buffer = [0; 1024];
            match tty.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(e),
            }
            if bytes.ends_with(b"\x1b\\") && replies(&bytes).len() == keys.len() {
                break;
            }
        }
        Ok(from_replies(&bytes))
    };
    attempt().unwrap_or_default()
}
#[cfg(not(unix))]
pub(crate) fn probe() -> Font {
    Font::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reply(key: &str, value: &str) -> String {
        format!(
            "\x1bP1+r{}={}\x1b\\",
            hex(&format!("kitty-query-{key}")),
            hex(value)
        )
    }
    #[test]
    fn terminal_postscript_name_and_points_are_preserved() {
        let s = reply("font_family", "JetBrainsMonoNFM-Regular")
            + &reply("font_size", "11")
            + &reply("dpi_y", "96");
        let f = from_replies(s.as_bytes());
        assert_eq!(f.name.as_deref(), Some("JetBrainsMonoNFM-Regular"));
        assert!((f.pixels.unwrap() - 14.666667).abs() < 0.001);
    }
    #[test]
    fn absent_invalid_and_unbounded_replies_fall_back() {
        for s in [
            String::new(),
            "\x1bP0+r1234\x1b\\".into(),
            reply("font_size", "NaN") + &reply("dpi_y", "96"),
        ] {
            assert!(from_replies(s.as_bytes()).pixels.is_none());
        }
        assert!(unhex("f").is_none());
        assert!(unhex(&"aa".repeat(513)).is_none());
        assert!(unhex("1b").is_none());
        assert!(unhex("é").is_none());
    }
}
