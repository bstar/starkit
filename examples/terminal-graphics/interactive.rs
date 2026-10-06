//! Interactive, application-independent consumer of the shared session API.
use starkit::terminal_graphics::{
    client::{run as client, Launch},
    protocol::*,
    session::{self, Controller},
};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// A generated preview keeps the shared example independent of local files.
pub fn preview(rect: Rect) -> Component {
    static PNG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let png = PNG.get_or_init(|| {
        let image = starkit::image::RgbaImage::from_fn(80, 48, |x, y| {
            let ridge = 18 + x.abs_diff(40) / 3;
            let color = if y >= ridge {
                [166, 227, 161, 255]
            } else {
                [137, 180, 250, 255]
            };
            starkit::image::Rgba(color)
        });
        starkit::terminal_graphics::assets::encode_png(&image)
            .expect("fixed demo surface is within PNG limits")
    });
    Component::Image {
        rect,
        id: "shared-demo-preview".into(),
        png: Some(png.clone()),
        scale: Default::default(),
        zoom: 100,
    }
}
pub fn handles() -> bool {
    std::env::args()
        .nth(1)
        .is_some_and(|s| s == "--interactive" || s == "--graphical-relay" || s == "--serve")
}
pub fn run() -> anyhow::Result<()> {
    let _log = if std::env::var_os("STAR_KIT_DEMO_DIR").is_some() {
        Some(starkit::logging::init(
            &starkit::paths::Paths::new(
                "star_kit_demo",
                "STAR_KIT_DEMO_DIR",
                "STAR_KIT_DEMO_CONFIG_DIR",
            ),
            true,
        )?)
    } else {
        None
    };
    let args = std::env::args().collect::<Vec<_>>();
    let root = std::env::home_dir()
        .ok_or_else(|| anyhow::anyhow!("No user home"))?
        .join(".local/starkit/graphical");
    if args[1] == "--interactive" {
        return client(Launch {
            executable: std::env::current_exe()?.to_string_lossy().into(),
            host: None,
            ssh_config: None,
            session: format!("demo-{}", std::process::id()),
            directory: None,
            attach_only: false,
            play: None,
        });
    }
    session::private_root(&root)?;
    let name = args
        .get(2)
        .ok_or_else(|| anyhow::anyhow!("Missing session"))?;
    if args[1] == "--serve" {
        return session::serve(&root, name, Demo::default());
    }
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
            if child.try_wait()?.is_some() {
                anyhow::bail!("Demo session failed");
            }
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(5),
                "Demo startup timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    session::relay(&socket)
}
#[derive(Default)]
struct Demo {
    cursor: usize,
    filter: String,
    menu: bool,
    tab: bool,
    quit: bool,
    marked: std::collections::HashSet<usize>,
}
impl Controller for Demo {
    fn tick(&mut self) {}
    fn closed(&self) -> bool {
        self.quit
    }
    fn shutdown(&mut self) {}
    fn input(&mut self, input: Input) {
        match input {
            Input::Key { code, .. } => match code.as_str() {
                "down" | "char:j" => self.cursor = (self.cursor + 1).min(99_999),
                "up" | "char:k" => self.cursor = self.cursor.saturating_sub(1),
                "tab" => self.tab = !self.tab,
                "char:c" => self.menu = !self.menu,
                "escape" => {
                    self.menu = false;
                    self.filter.clear();
                }
                "char: " => {
                    if !self.marked.insert(self.cursor) {
                        self.marked.remove(&self.cursor);
                    }
                }
                "char:q" => self.quit = true,
                "backspace" => {
                    self.filter.pop();
                }
                key => {
                    if let Some(c) = key.strip_prefix("char:") {
                        if self.filter.len() < 1024 {
                            self.filter.push_str(c);
                        }
                    }
                }
            },
            Input::Paste { text } => self.filter = text.chars().take(1024).collect(),
            Input::Pointer {
                action,
                button,
                x,
                y,
                ..
            } if action == "down" => {
                if button == 1 {
                    self.menu = true;
                } else if y < 5 {
                    self.tab = x > 25;
                } else if y >= 8 {
                    self.cursor = self.cursor.saturating_sub(10) + (y - 8) as usize;
                }
            }
            _ => {}
        }
    }
    fn scene(&mut self, v: Viewport) -> Scene {
        let area = starkit::ratatui::layout::Rect::new(0, 0, v.columns, v.rows);
        let mut buffer = starkit::ratatui::buffer::Buffer::empty(area);
        buffer.set_style(
            area,
            starkit::ratatui::style::Style::default()
                .bg(starkit::ratatui::style::Color::Rgb(30, 30, 46))
                .fg(starkit::ratatui::style::Color::Rgb(205, 214, 244)),
        );
        let mut scene = Scene::from_buffer(&buffer, v, 0);
        // This demo has no compatibility text; its content comes from the
        // shared components in both pixel and cell presentation.
        scene.spans.clear();
        scene.interaction = self.cursor as u64;
        scene.accent = "#89b4fa".into();
        scene.border = "#45475a".into();
        scene.components.push(Component::Panel {
            rect: Rect {
                x: 1,
                y: 1,
                width: v.columns.saturating_sub(2),
                height: v.rows.saturating_sub(2),
            },
            active: true,
        });
        for (i, label) in ["Shared components ×", "100,000 rows ×"]
            .into_iter()
            .enumerate()
        {
            scene.components.push(Component::Tab {
                number: None,
                close: None,
                rect: Rect {
                    x: 3 + i as u16 * 24,
                    y: 2,
                    width: 22,
                    height: 3,
                },
                label: label.into(),
                active: self.tab == (i == 1),
            });
        }
        scene.components.push(Component::TextField {
            rect: Rect {
                x: 3,
                y: 6,
                width: v.columns.saturating_sub(6),
                height: 1,
            },
            text: if self.filter.is_empty() {
                "Type a filter · arrows move · space marks · c opens menu · q quits".into()
            } else {
                self.filter.clone()
            },
            caret: self.filter.len(),
            secret: false,
        });
        let top = self.cursor.saturating_sub(10);
        let preview_width = if v.columns >= 60 && v.rows >= 20 {
            22
        } else {
            0
        };
        if preview_width != 0 {
            scene.components.push(preview(Rect {
                x: v.columns - 20,
                y: 8,
                width: 16,
                height: 8.min(v.rows.saturating_sub(12)),
            }));
        }
        for y in 8..v.rows.saturating_sub(4) {
            let index = top + usize::from(y - 8);
            scene.components.push(Component::ListRow {
                rect: Rect {
                    x: 3,
                    y,
                    width: v.columns.saturating_sub(6 + preview_width),
                    height: 1,
                },
                label: format!("Shared row {index} · {}", self.filter),
                icon: if index.is_multiple_of(3) {
                    "folder"
                } else {
                    "file"
                }
                .into(),
                foreground: scene.foreground.clone(),
                background: if index == self.cursor {
                    "#45475a"
                } else {
                    "#1e1e2e"
                }
                .into(),
                selected: index == self.cursor,
                marked: self.marked.contains(&index),
                marking: !self.marked.is_empty(),
            });
        }
        scene.components.push(Component::Meter {
            rect: Rect {
                x: 3,
                y: v.rows.saturating_sub(3),
                width: 20,
                height: 1,
            },
            value: (self.cursor % 1000) as u16,
            foreground: scene.accent.clone(),
            background: scene.border.clone(),
        });
        if self.menu {
            let rect = Rect {
                x: 5,
                y: 9,
                width: 35,
                height: 6,
            };
            for y in rect.y..rect.y + rect.height {
                scene.spans.push(Span {
                    x: rect.x,
                    y,
                    text: " ".repeat(35),
                    foreground: scene.foreground.clone(),
                    background: "#313244".into(),
                    bold: false,
                });
            }
            scene.spans.push(Span {
                x: 7,
                y: 11,
                text: "Space · mark/unmark".into(),
                foreground: scene.foreground.clone(),
                background: "#313244".into(),
                bold: false,
            });
            scene.spans.push(Span {
                x: 7,
                y: 13,
                text: "Esc · close menu".into(),
                foreground: scene.foreground.clone(),
                background: "#313244".into(),
                bold: false,
            });
            scene
                .components
                .retain(|c| !matches!(c,Component::ListRow{rect:r,..} if r.y>=9 && r.y<=15));
            scene.components.push(Component::Menu { rect });
        }
        scene
    }
}
