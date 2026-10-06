//! Local-only, fixed Kitty window controls. Never runs a command from the host.
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
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
