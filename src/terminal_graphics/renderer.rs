//! Browser lifecycle and Kitty image presentation. No application commands here.
use std::fs;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crossbeam_channel::{bounded, Receiver};
use serde::Deserialize;

use super::protocol::{read_message, write_message, Scene};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RenderMessage {
    Ready,
    Frame {
        revision: u64,
        generation: u64,
        width: u32,
        height: u32,
        png: String,
    },
    Error {
        message: String,
    },
}

pub struct Renderer {
    child: Child,
    input: Option<ChildStdin>,
    pub output: Receiver<RenderMessage>,
    directory: PathBuf,
}
impl Renderer {
    pub fn spawn() -> Result<Self> {
        let root = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = root.join(format!("starkit-graphics-{}-{stamp}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&directory)?;
        fs::write(directory.join("main.cjs"), super::RUNTIME_MAIN)?;
        fs::write(directory.join("index.html"), super::RUNTIME_HTML)?;
        fs::write(directory.join("preload.cjs"), super::RUNTIME_PRELOAD)?;
        let executable =
            std::env::var_os("STAR_GRAPHICS_ELECTRON").unwrap_or_else(|| "electron".into());
        let log = fs::File::create(directory.join("renderer.log"))?;
        let mut command = Command::new(executable);
        #[cfg(target_os = "linux")]
        if let Ok(platform) = std::env::var("STAR_GRAPHICS_PLATFORM") {
            if matches!(platform.as_str(), "x11" | "wayland") {
                command.arg(format!("--ozone-platform={platform}"));
            }
        }
        let child = command
            .arg(directory.join("main.cjs"))
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(e)
                    .context("Install the graphical runtime or set STAR_GRAPHICS_ELECTRON");
            }
        };
        let input = child.stdin.take();
        let stdout = child.stdout.take().context("renderer stdout unavailable")?;
        let (tx, rx) = bounded(2);
        let drop_old = rx.clone();
        std::thread::Builder::new()
            .name("star-graphics-frames".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let message = match read_message::<RenderMessage>(&mut reader) {
                        Ok(Some(m)) => m,
                        Ok(None) => break,
                        Err(error) => RenderMessage::Error {
                            message: format!("Invalid renderer response: {error}. For npm Electron, download the runtime with install-electron before launching."),
                        },
                    };
                    let error = matches!(message, RenderMessage::Error { .. });
                    if let Err(crossbeam_channel::TrySendError::Full(message)) =
                        tx.try_send(message)
                    {
                        let _ = drop_old.try_recv();
                        let _ = tx.try_send(message);
                    }
                    if error {
                        break;
                    }
                }
            })?;
        let mut renderer = Self {
            child,
            input,
            output: rx,
            directory,
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match renderer.output.recv_timeout(Duration::from_millis(100)) {
                Ok(RenderMessage::Ready) => return Ok(renderer),
                Ok(RenderMessage::Error { message }) => bail!("Graphical renderer: {message}"),
                _ => {}
            }
            if let Some(status) = renderer.child.try_wait()? {
                let log =
                    fs::read_to_string(renderer.directory.join("renderer.log")).unwrap_or_default();
                bail!(
                    "Graphical renderer exited ({status}): {}",
                    log.chars().take(2000).collect::<String>()
                );
            }
            if Instant::now() > deadline {
                bail!("Graphical renderer did not start within 20 seconds");
            }
        }
    }
    pub fn scene(&mut self, scene: &Scene) -> Result<()> {
        write_message(
            &serde_json::json!({"type":"scene","scene":scene}),
            self.input.as_mut().context("renderer closed")?,
        )?;
        Ok(())
    }
    pub fn clipboard(&mut self, text: &str) -> Result<()> {
        write_message(
            &serde_json::json!({"type":"clipboard","text":text}),
            self.input.as_mut().context("renderer closed")?,
        )?;
        Ok(())
    }
    pub fn alive(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_millis(500);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// A stable placement is replaced atomically. Never clear between frames.
pub struct KittyPresenter {
    next: u32,
    previous: Option<u32>,
    cached: Option<(String, u16, u16)>,
}
impl Default for KittyPresenter {
    fn default() -> Self {
        Self {
            next: 0x534b0000,
            previous: None,
            cached: None,
        }
    }
}
impl KittyPresenter {
    pub fn present(
        &mut self,
        png: &str,
        columns: u16,
        rows: u16,
        out: &mut impl Write,
    ) -> io::Result<usize> {
        if png.len() > 15_000_000
            || !png
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PNG payload",
            ));
        }
        if self
            .cached
            .as_ref()
            .is_some_and(|(old, cols, lines)| old == png && *cols == columns && *lines == rows)
        {
            // Still acknowledge this scene revision: its input targets may
            // change even when its pixels do not.
            return Ok(0);
        }
        self.next = self.next.wrapping_add(1);
        let id = self.next;
        write!(out, "\x1b[?2026h\x1b[H")?;
        let chunks: Vec<_> = png.as_bytes().chunks(4096).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            let more = usize::from(index + 1 < chunks.len());
            if index == 0 {
                write!(
                    out,
                    "\x1b_Ga=T,f=100,t=d,i={id},p=1,q=2,C=1,c={columns},r={rows},m={more};"
                )?;
            } else {
                write!(out, "\x1b_Gm={more};")?;
            }
            out.write_all(chunk)?;
            out.write_all(b"\x1b\\")?;
        }
        if let Some(old) = self.previous {
            write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?;
        }
        out.write_all(b"\x1b[?2026l")?;
        out.flush()?;
        self.previous = Some(id);
        self.cached = Some((png.to_owned(), columns, rows));
        Ok(png.len())
    }
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.cached = None;
        if let Some(id) = self.previous.take() {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?;
        }
        out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_pixels_reuse_placement_but_geometry_and_cleanup_invalidate_it() {
        let mut p = KittyPresenter::default();
        let mut out = Vec::new();
        assert_eq!(p.present("AAAA", 80, 24, &mut out).unwrap(), 4);
        out.clear();
        assert_eq!(p.present("AAAA", 80, 24, &mut out).unwrap(), 0);
        assert!(out.is_empty());
        assert_eq!(p.present("AAAA", 81, 24, &mut out).unwrap(), 4);
        assert!(!out.is_empty());
        p.clear(&mut out).unwrap();
        out.clear();
        assert_eq!(p.present("AAAA", 81, 24, &mut out).unwrap(), 4);
        assert!(!out.is_empty());
    }

    #[test]
    fn replaces_before_retiring_and_rejects_escape_injection() {
        let mut p = KittyPresenter::default();
        let mut out = vec![];
        p.present("AAAA", 80, 24, &mut out).unwrap();
        out.clear();
        p.present("BBBB", 80, 24, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.find("a=T").unwrap() < s.find("a=d").unwrap());
        assert!(p.present("\x1b", 80, 24, &mut vec![]).is_err());
    }
}
