//! The application describes content and commands; KIT supplies both presentations.
use starkit::{
    theme::{Theme, ThemeFile, BUILTINS},
    ui::{
        command::{self, Command, Registry},
        Align, Column, Event, Heading, Item, Node, Runtime, Size, View, Viewport, Widget, Wrap,
    },
};

pub struct Gallery {
    name: String,
    selected: Option<starkit::ui::Id>,
    playing: bool,
    volume: u16,
    modal: bool,
    editing: bool,
    message: String,
    password: String,
    quit: bool,
    status: String,
    theme_id: String,
    choosing_theme: bool,
}
impl Default for Gallery {
    fn default() -> Self {
        Self {
            theme_id: starkit::theme::DEFAULT_ID.into(),
            choosing_theme: false,
            name: String::new(),
            selected: Some("track-1".into()),
            playing: false,
            volume: 65,
            modal: false,
            editing: false,
            message: "First line\nSecond line".into(),
            password: "private-fixture".into(),
            quit: false,
            status: "Ready · F1 Help · Alt+T Theme · F9 Cells / Graphics".into(),
        }
    }
}
impl Gallery {
    pub fn view(&self) -> View {
        let toolbar = Node::row(
            "toolbar",
            vec![
                Node::button("play", "play"),
                Node::button("settings", "settings"),
                Node::button("help", "kit.help"),
                Node::button("quit", "kit.quit"),
            ],
        )
        .height(Size::fixed(1))
        .wrap(Wrap::AtMinimum);
        let library = Node::pane(
            "library",
            Heading::new("Library").status("3 items"),
            Node::column(
                "library-content",
                vec![
                    Node::label("filter-label", "Filter"),
                    Node::field("filter", &self.name),
                    Node::table_with_columns(
                        "tracks",
                        vec![
                            Column::new("Title"),
                            Column::new("Time")
                                .width(Size::fixed(6))
                                .align(Align::Right),
                        ],
                        [
                            ("track-1", "First light", "3:04"),
                            ("track-2", "日本語 · Evening", "12:09"),
                            ("track-3", "A longer title to exercise clipping", "1:42"),
                        ]
                        .into_iter()
                        .map(|(id, title, duration)| Item {
                            columns: vec![title.into(), duration.into()],
                            ..Item::new(id, "")
                        })
                        .collect(),
                        self.selected.clone(),
                    ),
                ],
            ),
        )
        .width(Size::fill(24));
        let preview = Node::pane("preview", Heading::new("Preview").status(if self.playing { "Playing" } else { "Paused" }), Node::column("preview-content", vec![
            Node::label("description", "One UI tree. Shared chrome, focus and commands."),
            Node::new("canvas", Widget::Custom { fallback: "Video / live canvas\nNo media source is attached.\nControls remain available in cells.".into(), revision: 0 }),
            Node::new("enabled", Widget::Checkbox { command: "play".into() }),
            Node::label("volume-label", "Volume"), Node::slider("volume", self.volume, 100),
            Node::label("buffer-label", "Buffered"), Node::meter("buffer", 72, 100),
        ])).width(Size::fill(24));
        let root = Node::column(
            "application",
            vec![
                toolbar,
                Node::row("workspace", vec![library, preview]).wrap(Wrap::AtMinimum),
                Node::new("status", Widget::Status(self.status.clone())),
            ],
        );
        let mut view = View::new(root);
        if self.choosing_theme {
            return view.modal(
                Node::pane(
                    "theme-dialog",
                    Heading::new("Theme"),
                    Node::column(
                        "theme-content",
                        vec![
                            Node::new(
                                "themes",
                                Widget::List {
                                    items: BUILTINS
                                        .iter()
                                        .map(|builtin| Item::new(builtin.id, builtin.id))
                                        .collect::<Vec<_>>()
                                        .into(),
                                    selected: Some(self.theme_id.clone().into()),
                                    columns: vec![],
                                },
                            ),
                            Node::button("theme-close", "close"),
                        ],
                    ),
                )
                .width(Size::fixed(40))
                .height(Size::fixed(20)),
            );
        }
        if self.modal {
            view.modal = Some(
                Node::pane(
                    "dialog",
                    Heading::new("KIT application contract"),
                    Node::column(
                        "dialog-content",
                        vec![
                            Node::table_with_columns(
                                "command-help",
                                vec![
                                    Column::new("Shortcut").width(Size::fixed(15)),
                                    Column::new("Action"),
                                ],
                                self.commands()
                                    .into_iter()
                                    .map(|command| Item {
                                        id: command.id,
                                        columns: vec![command.shortcuts.join(" / "), command.label],
                                        depth: 0,
                                        checked: false,
                                        enabled: command.enabled,
                                    })
                                    .collect(),
                                None,
                            ),
                            Node::button("close", "close"),
                        ],
                    ),
                )
                .width(Size::fixed(48))
                .height(Size::fixed(12)),
            );
        }
        if self.editing {
            view.modal = Some(
                Node::pane(
                    "controls-dialog",
                    Heading::new("Text controls"),
                    Node::column(
                        "controls-content",
                        vec![
                            Node::label("composer-label", "Message · Alt+Enter for a new line"),
                            Node::multiline("composer", &self.message),
                            Node::label("password-label", "Password"),
                            Node::password("password", &self.password),
                            Node::new("editor-status", Widget::Status(self.status.clone())),
                            Node::button("editor-close", "close"),
                        ],
                    ),
                )
                .width(Size::fixed(48))
                .height(Size::fixed(18)),
            );
        }
        view
    }
    pub fn commands(&self) -> Vec<Command> {
        let mut commands = command::standard();
        commands.extend([
            Command::new("play", if self.playing { "Pause" } else { "Play" })
                .shortcut("ctrl+p")
                .checked(self.playing),
            Command::new("settings", "Settings…").shortcut("alt+s"),
            Command::new("close", "Close"),
        ]);
        commands
    }
    pub fn event(&mut self, event: Event) {
        match event {
            Event::Command(id) => match id.0.as_str() {
                "play" => {
                    self.playing = !self.playing;
                    self.status = if self.playing { "Playing" } else { "Paused" }.into();
                }
                "settings" => self.editing = true,
                "kit.help" => {
                    self.editing = false;
                    self.modal = true;
                }
                "close" => {
                    self.modal = false;
                    self.choosing_theme = false;
                    self.editing = false;
                }
                "kit.theme" => self.choosing_theme = true,
                "kit.quit" => self.quit = true,
                _ => {}
            },
            Event::Adjusted { id, value, .. } if id.0 == "volume" => {
                self.volume = value;
                self.status = format!("Volume {value}%");
            }
            Event::CancelModal => {
                self.modal = false;
                self.choosing_theme = false;
                self.editing = false;
            }
            Event::Selected { id, item } if id.0 == "themes" => self.theme_id = item.0,
            Event::Activated { id, item } if id.0 == "themes" => {
                self.theme_id = item.0;
                self.choosing_theme = false;
            }
            Event::Activated { id, item } if id.0 == "tracks" => {
                self.status = format!("Playing {}", item.0);
                self.selected = Some(item);
                self.playing = true;
            }
            Event::Changed { id, value } if id.0 == "filter" => self.name = value,
            Event::Changed { id, value } if id.0 == "composer" => self.message = value,
            Event::Changed { id, value } if id.0 == "password" => self.password = value,
            Event::Submitted { id, .. } if id.0 == "composer" => {
                self.status = format!("Submitted {} lines", self.message.lines().count());
            }
            Event::Selected { id, item } if id.0 == "tracks" => {
                self.status = format!("Selected {}", item.0);
                self.selected = Some(item);
            }
            _ => {}
        }
    }
}
pub fn theme(id: &str) -> anyhow::Result<Theme> {
    let builtin = BUILTINS
        .iter()
        .find(|b| b.id == id)
        .ok_or_else(|| anyhow::anyhow!("Unknown built-in theme {id}"))?;
    Ok(Theme::resolve(&ThemeFile::parse(builtin.toml)?))
}
pub fn runtime(id: &str) -> anyhow::Result<Runtime> {
    let gallery = Gallery {
        theme_id: id.into(),
        ..Gallery::default()
    };
    Runtime::new(
        gallery.view(),
        Registry::new(gallery.commands())?,
        theme(id)?,
    )
}
pub fn cell_text(id: &str, width: u16, height: u16) -> anyhow::Result<String> {
    let buffer = runtime(id)?.render_cells(Viewport::cells(width, height))?;
    Ok((0..height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += starkit::wrap::width_of(symbol).max(1);
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n"))
}
impl starkit::ui::application::Application for Gallery {
    fn view(&self) -> View {
        self.view()
    }
    fn commands(&self) -> Vec<Command> {
        self.commands()
    }
    fn event(&mut self, event: Event) {
        self.event(event);
    }
    fn theme(&self) -> Option<Theme> {
        theme(&self.theme_id).ok()
    }
    fn closed(&self) -> bool {
        self.quit
    }
}
#[cfg(feature = "native")]
pub fn native(id: &str) -> anyhow::Result<()> {
    let paths = starkit::paths::Paths::new(
        "starkit-starter",
        "STAR_KIT_STARTER_DIR",
        "STAR_KIT_STARTER_CONFIG_DIR",
    );
    let _log = starkit::logging::init(&paths, true)?;
    use starkit::terminal_graphics::client::{
        self, Launch, PresentationOptions, PresentationPreference,
    };
    use starkit::{terminal_graphics::application::Application, ui::application::Host};
    let args: Vec<_> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].clone())
    };
    let preference = PresentationPreference {
        cells: args.iter().any(|arg| arg == "--cells"),
        path: paths.config_dir()?.join("presentation.toml"),
    };
    if let Some(host) = option("--ssh") {
        return client::run_with_preference(
            Launch {
                executable: option("--remote-executable")
                    .unwrap_or_else(|| "starkit-starter".into()),
                host: Some(host),
                ssh_config: option("--ssh-config").map(Into::into),
                session: option("--session").unwrap_or_else(|| "kit-gallery".into()),
                directory: None,
                attach_only: args.iter().any(|arg| arg == "--attach-only"),
                play: None,
            },
            |_| None,
            || None,
            PresentationOptions::default(),
            preference,
        );
    }
    let id = id.to_string();
    Application::new("kit-gallery")?
        .with_presentation(preference)
        .run(move |domain| {
            let selected = domain
                .windows(2)
                .find(|pair| pair[0] == "--theme")
                .map(|pair| pair[1].as_str())
                .unwrap_or(&id);
            Host::new(
                Gallery {
                    theme_id: selected.into(),
                    ..Gallery::default()
                },
                theme(selected)?,
            )
        })
}
#[cfg(feature = "native")]
pub fn png(id: &str, path: &str) -> anyhow::Result<()> {
    let mut ui = runtime(id)?;
    let viewport = starkit::terminal_graphics::Viewport::default();
    let scene = ui.render_scene(viewport, false)?;
    starkit::terminal_graphics::renderer::render_reference(&scene)?.save(path)?;
    Ok(())
}
pub fn cells(id: &str) -> anyhow::Result<()> {
    starkit::ui::application::Host::new(
        Gallery {
            theme_id: id.into(),
            ..Gallery::default()
        },
        theme(id)?,
    )?
    .run_cells()
}

impl starkit::ui::content::Content for Gallery {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_dialogs_render_over_the_application_in_both_presentations() {
        for command in ["kit.help", "settings", "kit.theme"] {
            let mut gallery = Gallery::default();
            gallery.event(Event::Command(command.into()));
            for (width, height) in [(700, 640), (336, 136), (12, 12)] {
                let mut ui = Runtime::new(
                    gallery.view(),
                    Registry::new(gallery.commands()).unwrap(),
                    theme(starkit::theme::DEFAULT_ID).unwrap(),
                )
                .unwrap();
                ui.render_surface(Viewport::graphics(width, height))
                    .unwrap()
                    .validate()
                    .unwrap();
                #[cfg(feature = "native")]
                ui.render_scene(
                    starkit::terminal_graphics::Viewport {
                        width: u32::from(width),
                        height: u32::from(height),
                        columns: (width / 8).max(1),
                        rows: (height / 24).max(1),
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
                ui.render_cells(Viewport::cells((width / 8).max(1), (height / 24).max(1)))
                    .unwrap();
            }
        }
    }
}
