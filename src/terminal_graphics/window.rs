//! Local-only, fixed Kitty window controls. Never runs a command from the host.
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct Window {
    sender: Sender<bool>,
    pub notices: Receiver<String>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Window {
    pub fn new() -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("star-kit-window-{}", std::process::id()));
        std::fs::create_dir(&root)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        let path = root.join("fullscreen.py");
        std::fs::write(
            &path,
            include_bytes!("../../integrations/kitty/fullscreen.py"),
        )?;
        let (sender, rx) = bounded(8);
        let (tx, notices) = bounded(4);
        let worker = std::thread::Builder::new().name("star-kitty-window".into()).spawn(move || {
            let mut entered = false;
            for enabled in rx {
                match operation(&path,enabled) {
                    Ok(()) => entered = enabled,
                    Err(error) => { tracing::warn!(%error, "Kitty fullscreen operation failed"); let _ = tx.try_send(format!("Desktop fullscreen unavailable: {error:#}. Use Kitty’s fullscreen shortcut.")); }
                }
            }
            if entered { let _ = operation(&path,false); }
            let _ = std::fs::remove_dir_all(root);
        })?;
        Ok(Self {
            sender,
            notices,
            worker: Some(worker),
        })
    }
    pub fn set(&self, enabled: bool) {
        let _ = self.sender.try_send(enabled);
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        let (tx, _) = bounded(1);
        drop(std::mem::replace(&mut self.sender, tx));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn operation(path: &PathBuf, enabled: bool) -> Result<()> {
    let id = std::env::var("KITTY_WINDOW_ID").context("Kitty window identity missing")?;
    anyhow::ensure!(id.parse::<u64>().is_ok(), "Invalid Kitty window identity");
    // A second reader on /dev/tty races crossterm: remote-control replies can
    // become Escape and text key events, exiting fullscreen and stopping video.
    // Socket controls have a private reply channel; terminal controls must be
    // one-way and use the application's serialized stdout writer.
    if std::env::var_os("KITTY_LISTEN_ON").is_none() {
        let wire = terminal_command(path, &id, enabled)?;
        let mut out = std::io::stdout().lock();
        out.write_all(&wire)?;
        out.flush()?;
        return Ok(());
    }
    let mut command = Command::new("kitten");
    command.arg("@");
    if let Ok(socket) = std::env::var("KITTY_LISTEN_ON") {
        command.args(["--to", &socket]);
    }
    let mut child = command
        .args(["kitten", "--match", &format!("id:{id}")])
        .arg(path)
        .arg(if enabled { "enter" } else { "leave" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Kitty control timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = child.wait_with_output()?;
    anyhow::ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(())
}

fn terminal_command(path: &std::path::Path, id: &str, enabled: bool) -> Result<Vec<u8>> {
    let id: u64 = id.parse().context("Invalid Kitty window identity")?;
    let command = serde_json::json!({
        "cmd": "kitten", "version": [0, 35, 0], "no_response": true,
        "kitty_window_id": id,
        "payload": {"kitten": path, "match": format!("id:{id}"),
            "args": [if enabled { "enter" } else { "leave" }]}
    });
    let mut wire = b"\x1bP@kitty-cmd".to_vec();
    serde_json::to_writer(&mut wire, &command)?;
    wire.extend_from_slice(b"\x1b\\");
    Ok(wire)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_fullscreen_never_requests_a_reply_on_the_input_stream() {
        for enabled in [true, false] {
            let wire =
                terminal_command(std::path::Path::new("/tmp/fullscreen.py"), "7", enabled).unwrap();
            assert!(wire.starts_with(b"\x1bP@kitty-cmd"));
            assert!(wire.ends_with(b"\x1b\\"));
            let command: serde_json::Value =
                serde_json::from_slice(&wire[12..wire.len() - 2]).unwrap();
            assert_eq!(command["no_response"], true);
            assert_eq!(command["kitty_window_id"], 7);
            assert_eq!(command["payload"]["match"], "id:7");
            assert_eq!(
                command["payload"]["args"][0],
                if enabled { "enter" } else { "leave" }
            );
        }
        assert!(
            terminal_command(std::path::Path::new("/tmp/fullscreen.py"), "7;other", true).is_err()
        );
    }
}
