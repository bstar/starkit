//! Colors reported by the presenting terminal, queried before UI input ownership.
use crate::theme::color::Rgb;

fn reply(bytes: &[u8]) -> Option<Rgb> {
    let start = bytes.windows(5).position(|w| w == b"\x1b]11;")? + 5;
    let value = &bytes[start..];
    let end = value
        .iter()
        .position(|b| *b == 7)
        .into_iter()
        .chain(value.windows(2).position(|w| w == b"\x1b\\"))
        .min()?;
    let value = std::str::from_utf8(value.get(..end)?).ok()?;
    if value.starts_with('#') {
        return Rgb::parse_hex(value).ok();
    }
    let mut channels = value.strip_prefix("rgb:")?.split('/');
    let channel = |text: &str| {
        if !(1..=4).contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(text, 16).ok()?;
        let max = (1u32 << (4 * text.len())) - 1;
        Some(((value * 255 + max / 2) / max) as u8)
    };
    let color = Rgb::new(
        channel(channels.next()?)?,
        channel(channels.next()?)?,
        channel(channels.next()?)?,
    );
    channels.next().is_none().then_some(color)
}

/// Query OSC 11 on the frontend terminal. No configuration files or remote
/// controller environment are used. Unsupported terminals return `None`.
/// Call before `term::init`; the bounded probe temporarily enters raw mode and restores it.
#[cfg(unix)]
pub fn background() -> Option<Rgb> {
    use crate::crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};
    use std::{
        io::{IsTerminal, Read, Write},
        os::unix::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };
    if crate::term::entered() || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()
    {
        return None;
    }
    let attempt = || -> std::io::Result<Option<Rgb>> {
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
        tty.write_all(b"\x1b]11;?\x1b\\")?;
        tty.flush()?;
        let deadline = Instant::now() + Duration::from_millis(200);
        let mut bytes = Vec::new();
        while Instant::now() < deadline && bytes.len() < 8192 {
            let mut buffer = [0; 1024];
            match tty.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => return Err(e),
            }
            if let Some(color) = reply(&bytes) {
                return Ok(Some(color));
            }
        }
        Ok(None)
    };
    attempt().ok().flatten()
}
#[cfg(not(unix))]
pub fn background() -> Option<Rgb> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_terminal_colors_and_rejects_incomplete_or_invalid_replies() {
        for text in [
            "\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\",
            "\x1b]11;rgb:1e/1e/2e\x07",
            "\x1b]11;#1e1e2e\x1b\\",
        ] {
            assert_eq!(reply(text.as_bytes()), Some(Rgb::new(30, 30, 46)));
        }
        assert_eq!(reply(b"\x1b]11;rgb:f/8/0\x07"), Some(Rgb::new(255, 136, 0)));
        for bytes in [
            b"\x1b]11;rgb:1111/2222/3333".as_slice(),
            b"\x1b]10;rgb:1111/2222/3333\x07",
            b"\x1b]11;rgb:zz/00/00\x07",
            b"\x1b]11;rgb:11111/0/0\x07",
            b"\x1b]11;rgb:1/2/3/4\x07",
        ] {
            assert_eq!(reply(bytes), None);
        }
    }
}
