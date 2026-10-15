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
        self.area = area;
        let layout = tree().layout(area).unwrap();
        self.focus.reconcile(&layout);
        let mut scene = self.visual.scene(v);
        for pane in layout.panes {
            if let Some(rect) = pane.area {
                let x = u32::from(rect.x) * v.width / u32::from(v.columns);
                let y = u32::from(rect.y) * v.height / u32::from(v.rows);
                let right = u32::from(rect.x + rect.width) * v.width / u32::from(v.columns);
                let bottom = u32::from(rect.y + rect.height) * v.height / u32::from(v.rows);
                let placement = starkit::terminal_graphics::placement::Placement::new(
                    rect.into(),
                    starkit::native_surface::PixelRect::new(
                        x as u16,
                        y as u16,
                        (right - x) as u16,
                        (bottom - y) as u16,
                    ),
                );
                let target = placement.target;
                let title = match pane.id {
                    Pane::Workspace => "WORKSPACE",
                    Pane::Preview => "PREVIEW",
                    Pane::Activity => "ACTIVITY",
                };
                let mut panel = self
                    .visual
                    .workspace_panel(target.width, target.height, title);
                if self.focus.active() == Some(pane.id) {
                    self.visual.focus(
                        &mut panel,
                        starkit::native_surface::PixelRect::new(0, 0, target.width, target.height),
                    );
                }
                if target.width > 24 && target.height > 64 {
                    self.visual.label(
                        &mut panel,
                        starkit::native_surface::PixelRect::new(
                            12,
                            40,
                            target.width - 24,
                            target.height - 52,
                        ),
                        "Tab changes focus. Alt+T themes. Resize to wrap panels. q exits.",
                    );
                }
                self.visual
                    .mount(&mut scene, panel, target.x, target.y)
                    .expect("bounded starter pane");
            }
        }
        self.theme_hits.clear();
        if let Some(picker) = &self.picker {
            let width = v.width.saturating_sub(32).min(760) as u16;
            let height = v.height.saturating_sub(32).min(540) as u16;
            let x = (v.width as u16 - width) / 2;
            let y = (v.height as u16 - height) / 2;
            let surface = picker.native(&self.visual, width, height);
            for hit in &surface.hits {
                let mut rect = hit.rect;
                rect.x += x;
                rect.y += y;
                self.theme_hits.push((rect, hit.action.clone()));
            }
            scene.components.push(Component::Menu {
                rect: starkit::native_surface::style::logical_rect(
                    starkit::native_surface::PixelRect::new(x, y, width, height),
                    v,
                ),
            });
            self.visual
                .mount(&mut scene, surface, x, y)
                .expect("bounded theme picker");
        }
        scene
    }
    fn capabilities(
        &mut self,
        capabilities: starkit::terminal_graphics::capabilities::Capabilities,
    ) {
        self.native_skins = capabilities.native_skins;
        self.visual = self
            .visual
            .clone()
            .with_native_skins(capabilities.native_skins);
    }
    fn input(&mut self, input: Input) {
        match input {
            Input::Key { code, modifiers } => {
                if starkit::theme::picker::Picker::is_shortcut(&code, modifiers) {
                    if self.picker.is_none() {
                        self.open_themes();
                    }
                } else if let Some(picker) = &mut self.picker {
                    let outcome = picker.handle_named(&code, modifiers);
                    self.theme_outcome(outcome);
                } else {
                    self.key(code.strip_prefix("char:").unwrap_or(&code));
                }
            }
            Input::Paste { text } => {
                if let Some(picker) = &mut self.picker {
                    picker.paste(&text);
                }
            }
            Input::Pointer {
                action,
                pixel: Some([x, y]),
                ..
            } => {
                if let Some(picker) = &mut self.picker {
                    if action == "scroll_up" || action == "scroll_down" {
                        picker.navigate(if action == "scroll_up" { -3 } else { 3 });
                    } else if action == "down" {
                        if let Some((_, action)) = self
                            .theme_hits
                            .iter()
                            .find(|(rect, _)| rect.contains(x as u16, y as u16))
                        {
                            let outcome = picker.action(action);
                            self.theme_outcome(outcome);
                        }
                    }
                }
            }
            _ => {}
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
