//! Same controller and layout through KIT's persistent local/SSH scene transport.
use super::*;
use starkit::terminal_graphics::{
    client::{self, Launch},
    protocol::{Component, Input, Scene, Viewport},
    session::{self, Controller},
};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
impl Controller for App {
    fn tick(&mut self) {}
    fn closed(&self) -> bool {
        self.quit
    }
    fn shutdown(&mut self) {}
    fn frame_interval(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn scene(&mut self, v: Viewport) -> Scene {
        let area = starkit::ratatui::layout::Rect::new(0, 0, v.columns, v.rows);
        let mut scene = Scene::from_buffer(&self.draw(area), v, 0);
        for pane in tree().layout(area).unwrap().panes {
            if let Some(rect) = pane.area {
                scene.components.push(Component::Panel {
                    rect: rect.into(),
                    active: self.focus.active() == Some(pane.id),
                });
            }
        }
        scene
    }
    fn input(&mut self, input: Input) {
        if let Input::Key { code, .. } = input {
            self.key(code.strip_prefix("char:").unwrap_or(&code));
        }
    }
}
pub fn run(args: &[String]) -> Result<()> {
    if args[0] == "--png" {
        let path = args
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("--png OUTPUT.png"))?;
        starkit::terminal_graphics::renderer::render_reference(
            &App::default().scene(Viewport::default()),
        )?
        .save(path)?;
        return Ok(());
    }
    let root = starkit::paths::Paths::new(
        "starkit_starter",
        "STAR_KIT_STARTER_DIR",
        "STAR_KIT_STARTER_CONFIG_DIR",
    )
    .runtime_dir()?;
    session::private_root(&root)?;
    let name = args.get(1).map(String::as_str).unwrap_or("starter");
    if args[0] == "--serve" {
        return session::serve(&root, name, App::default());
    }
    if args[0] == "--graphical-relay" {
        let socket = session::socket_path(&root, name)?;
        if !socket.exists() {
            use std::os::unix::process::CommandExt;
            let mut child = Command::new(std::env::current_exe()?)
                .args(["--serve", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()?;
            let start = Instant::now();
            while !socket.exists() {
                anyhow::ensure!(
                    child.try_wait()?.is_none() && start.elapsed() < Duration::from_secs(5),
                    "Starter session failed to start"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        return session::relay(&socket);
    }
    let host = if args[0] == "--ssh" {
        Some(
            args.get(1)
                .ok_or_else(|| anyhow::anyhow!("--ssh HOST [REMOTE_EXECUTABLE]"))?
                .clone(),
        )
    } else {
        None
    };
    let executable = if host.is_some() {
        args.get(2)
            .cloned()
            .unwrap_or_else(|| "starkit-starter".into())
    } else {
        std::env::current_exe()?.to_string_lossy().into_owned()
    };
    client::run(Launch {
        executable,
        host,
        ssh_config: None,
        session: format!("starter-{}", std::process::id()),
        directory: None,
        attach_only: false,
        play: None,
    })
}
