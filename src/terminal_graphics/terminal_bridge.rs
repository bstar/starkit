//! Optional terminal-owned frontend, negotiated over an existing SSH TTY.
//!
//! The terminal integration starts a fixed local Rust executable. The host
//! supplies scenes and receives inputs; it never supplies local commands or paths.
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::{fs::OpenOptionsExt, net::UnixStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use super::protocol::{read_message, write_message, ClientMessage, ServerMessage, MAX_MESSAGE};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};

pub struct Attachment {
    nonce: String,
    tty: Option<File>,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        let _ = emit(&self.nonce, "stop", None);
        let _ = crate::crossterm::terminal::disable_raw_mode();
    }
}
fn emit(nonce: &str, op: &str, data: Option<&str>) -> Result<()> {
    let value =
        serde_json::to_vec(&serde_json::json!({"v": 1, "nonce": nonce, "op": op, "data": data}))?;
    let mut out = io::stdout().lock();
    write!(
        out,
        "\x1b]1337;SetUserVar=star_kit={}\x1b\\",
        STANDARD.encode(value)
    )?;
    out.flush()?;
    Ok(())
}
/// Negotiate a locally installed terminal frontend before any rendering or probes.
/// Unsupported terminals retain the ordinary in-terminal frontend.
pub fn probe() -> Result<Option<Attachment>> {
    use io::IsTerminal;
    if std::env::var_os("SSH_CONNECTION").is_none()
        || !io::stdin().is_terminal()
        || !io::stdout().is_terminal()
    {
        return Ok(None);
    }
    let mut random = [0; 24];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let nonce = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let tty = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open("/dev/tty")?;
    crate::crossterm::terminal::enable_raw_mode()?;
    let mut attachment = Attachment {
        nonce,
        tty: Some(tty),
    };
    emit(&attachment.nonce, "probe", None)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut input = Lines::default();
    let expected = format!("STAR_KIT_BRIDGE {} ready", attachment.nonce);
    while Instant::now() < deadline {
        match input.next(attachment.tty.as_mut().unwrap())? {
            Some(line) if line == expected => return Ok(Some(attachment)),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    drop(attachment);
    Ok(None)
}
#[derive(Default)]
struct Lines {
    pending: Vec<u8>,
}
impl Lines {
    fn next(&mut self, tty: &mut File) -> io::Result<Option<String>> {
        loop {
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                let bytes = self.pending.drain(..=end).collect::<Vec<_>>();
                return String::from_utf8(bytes[..end].to_vec())
                    .map(Some)
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Invalid terminal bridge UTF-8")
                    });
            }
            let mut chunk = [0; 8192];
            match tty.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Terminal closed",
                    ))
                }
                Ok(n) => {
                    if self.pending.len().saturating_add(n) > MAX_MESSAGE + 128 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Terminal bridge input exceeds limit",
                        ));
                    }
                    self.pending.extend_from_slice(&chunk[..n]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}
impl Attachment {
    /// Attach to the application's existing persistent session. No new SSH connection.
    pub fn relay(self, path: &std::path::Path) -> Result<()> {
        self.relay_with_play(path, None)
    }
    /// Open a host movie after the existing terminal frontend attaches.
    pub fn relay_with_play(
        mut self,
        path: &std::path::Path,
        mut play: Option<String>,
    ) -> Result<()> {
        let socket = UnixStream::connect(path).context("Attach terminal bridge session")?;
        let mut launch_socket = socket.try_clone()?;
        let mut input_socket = socket.try_clone()?;
        let mut tty = self.tty.take().unwrap();
        let nonce = self.nonce.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let input = std::thread::spawn(move || {
            let prefix = format!("STAR_KIT_BRIDGE {nonce} ");
            let mut lines = Lines::default();
            let result = (|| -> Result<()> {
                while !stopped.load(Ordering::Acquire) {
                    if let Some(line) = lines.next(&mut tty)? {
                        if let Some(data) = line.strip_prefix(&prefix) {
                            if data == "gone" {
                                break;
                            }
                            let message: ClientMessage = serde_json::from_str(data)?;
                            write_message(&message, &mut input_socket)?;
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                Ok(())
            })();
            let _ = input_socket.shutdown(std::net::Shutdown::Both);
            result
        });
        let result = (|| -> Result<()> {
            emit(&self.nonce, "start", None)?;
            let mut reader = BufReader::new(socket);
            while let Some(message) = read_message::<ServerMessage>(&mut reader)? {
                if let ServerMessage::Scene { scene } = &message {
                    if let Some(path) = play.take() {
                        write_message(
                            &ClientMessage::Input {
                                id: 0,
                                revision: scene.revision,
                                generation: scene.viewport.generation,
                                input: super::protocol::Input::Play { path },
                            },
                            &mut launch_socket,
                        )?;
                    }
                }
                let bytes = serde_json::to_vec(&message)?;
                for chunk in bytes.chunks(3072) {
                    emit(&self.nonce, "data", Some(&STANDARD.encode(chunk)))?;
                }
                emit(&self.nonce, "end", None)?;
                if matches!(message, ServerMessage::Closed) {
                    break;
                }
            }
            Ok(())
        })();
        stop.store(true, Ordering::Release);
        let input_result = input
            .join()
            .map_err(|_| anyhow::anyhow!("Terminal bridge reader stopped"))?;
        result.and(input_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_lines_preserve_boundaries_and_report_closed_terminal() {
        let mut lines = Lines {
            pending: b"one\ntwo\npartial".to_vec(),
        };
        let mut null = File::open("/dev/null").unwrap();
        assert_eq!(lines.next(&mut null).unwrap(), Some("one".into()));
        assert_eq!(lines.next(&mut null).unwrap(), Some("two".into()));
        assert!(lines.next(&mut null).is_err());
        assert_eq!(lines.pending, b"partial");
    }
}
