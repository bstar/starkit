//! A synchronous, bounded capability query. No reader survives its return.
#[cfg(unix)]
use super::ProtocolType;
use super::{FontSize, Picker};
use std::io;

pub(super) struct Detected {
    pub picker: Picker,
    pub cell: Option<FontSize>,
}

#[cfg(unix)]
const MAX_REPLY_BYTES: usize = 8192;

#[cfg(unix)]
#[derive(Default)]
struct Answers {
    parser: ratatui_image::picker::cap_parser::Parser,
    prefix: Vec<u8>,
    protocol: Option<ProtocolType>,
    cell: Option<FontSize>,
    bytes: usize,
    completed: bool,
}
#[cfg(unix)]
impl Answers {
    fn push(&mut self, byte: u8) -> bool {
        use ratatui_image::picker::cap_parser::Response;
        if self.completed || self.bytes >= MAX_REPLY_BYTES {
            return true;
        }
        self.bytes += 1;
        if byte == 0x1b {
            self.prefix.clear();
        }
        if self.prefix.len() < 4 {
            self.prefix.push(byte);
        }
        for response in self.parser.push(char::from(byte)) {
            match response {
                Response::Kitty => self.protocol = Some(ProtocolType::Kitty),
                Response::Sixel if self.protocol.is_none() => {
                    self.protocol = Some(ProtocolType::Sixel)
                }
                // The upstream parser also accepts CSI 4 (whole-window pixels).
                // Only CSI 6 answers our per-cell measurement query.
                Response::CellSize(Some((width, height)))
                    if self.prefix == b"\x1b[6;" && width > 0 && height > 0 =>
                {
                    self.cell = Some(FontSize { width, height });
                }
                Response::Status => self.completed = true,
                _ => {}
            }
        }
        self.completed || self.bytes >= MAX_REPLY_BYTES
    }
}

#[cfg(unix)]
pub(super) fn query() -> io::Result<Detected> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt, time::Duration};
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open("/dev/tty")?;
    let timeout = std::env::var("STAR_GRAPHICS_PROBE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ms| (50..=10_000).contains(ms))
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(2));
    let original = rustix::termios::tcgetattr(&tty)?;
    let mut raw = original.clone();
    raw.make_raw();
    struct Restore(std::fs::File, rustix::termios::Termios);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ =
                rustix::termios::tcsetattr(&self.0, rustix::termios::OptionalActions::Now, &self.1);
        }
    }
    // Install ownership before changing any attributes, including failure paths.
    // Restore the actual attributes without touching crossterm's mode cache.
    let mut restore = Restore(tty, original);
    rustix::termios::tcsetattr(&restore.0, rustix::termios::OptionalActions::Now, &raw)?;
    let result = exchange(&mut restore.0, timeout);
    // Report a restoration failure; Drop also retries on error or unwinding.
    rustix::termios::tcsetattr(
        &restore.0,
        rustix::termios::OptionalActions::Now,
        &restore.1,
    )?;
    result
}

#[cfg(not(unix))]
pub(super) fn query() -> io::Result<Detected> {
    // The supported native targets are Unix. Never use an unbounded stdio
    // reader on another platform merely to improve a capability guess.
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Bounded graphics probing is unavailable",
    ))
}

#[cfg(unix)]
fn exchange(tty: &mut std::fs::File, timeout: std::time::Duration) -> io::Result<Detected> {
    use ratatui_image::picker::cap_parser::{Parser, QueryStdioOptions};
    use std::{
        io::{Read, Write},
        time::{Duration, Instant},
    };
    let mut options = QueryStdioOptions::default();
    if std::env::var_os("WEZTERM_EXECUTABLE").is_some()
        || std::env::var_os("KONSOLE_VERSION").is_some()
    {
        // These terminals use their established iTerm2 fallback instead.
        options.blacklist_protocols = vec![ProtocolType::Kitty, ProtocolType::Sixel];
    }
    let query = Parser::query(super::behind_multiplexer(), options);
    let deadline = Instant::now() + timeout;
    let mut remaining = query.as_bytes();
    while !remaining.is_empty() && Instant::now() < deadline {
        match tty.write(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => remaining = &remaining[n..],
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    if !remaining.is_empty() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    let mut answers = Answers::default();
    // One-byte reads stop exactly at the final status report, leaving any
    // subsequent keyboard input in the terminal for the application.
    while Instant::now() < deadline {
        let mut byte = [0];
        match tty.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if answers.push(byte[0]) {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let cell = answers.cell.or_else(|| {
        crate::crossterm::terminal::window_size()
            .ok()
            .and_then(|size| super::cell_size(size.columns, size.rows, size.width, size.height))
    });
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(cell.unwrap_or(FontSize {
        width: 10,
        height: 20,
    }));
    if let Some(protocol) = answers.protocol {
        picker.set_protocol_type(protocol);
    }
    tracing::debug!(
        completed = answers.completed,
        bytes = answers.bytes,
        elapsed_ms = timeout
            .saturating_sub(deadline.saturating_duration_since(Instant::now()))
            .as_millis(),
        "Graphics capability query finished"
    );
    Ok(Detected { picker, cell })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use proptest::prelude::*;
    #[test]
    fn only_cell_reports_can_establish_cell_geometry() {
        let mut answers = Answers::default();
        for byte in b"\x1b[4;720;900t\x1b[6;0;9t\x1b[6;18;0t" {
            answers.push(*byte);
        }
        assert!(answers.cell.is_none());
        for byte in b"\x1b[6;18;9t" {
            answers.push(*byte);
        }
        assert_eq!(answers.cell.map(|s| (s.width, s.height)), Some((9, 18)));
    }
    proptest! {
        #[test]
        fn arbitrary_replies_are_bounded_and_never_create_zero_cell_geometry(bytes in prop::collection::vec(any::<u8>(), 0..16_384)) {
            let mut answers = Answers::default();
            for byte in bytes { answers.push(byte); }
            prop_assert!(answers.bytes <= MAX_REPLY_BYTES);
            prop_assert!(answers.prefix.len() <= 4);
            prop_assert!(answers.cell.is_none_or(|cell| cell.width > 0 && cell.height > 0));
            if answers.completed || answers.bytes == MAX_REPLY_BYTES {
                let count = answers.bytes;
                prop_assert!(answers.push(b'q'));
                prop_assert_eq!(answers.bytes, count);
            }
        }
    }
}
