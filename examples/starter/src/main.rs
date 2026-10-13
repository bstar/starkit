//! Copyable independent consumer. KIT owns presentation; the application owns state.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use starkit::{
    chrome::frame::{self, Frame, NO_WORDS},
    diagnostics::{Action, Replay},
    panes::{Focus, Node, Overflow, Size, Wrap},
    presentation::{Environment, Observed, Profile, Tier},
    ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::Style,
        widgets::{Paragraph, Widget},
    },
};
#[cfg(all(feature = "native", unix))]
mod native;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Pane {
    Workspace,
    Preview,
    Activity,
}
fn tree() -> Node<Pane> {
    Node::column(vec![
        Node::pane(Pane::Workspace)
            .height(Size::fill(6))
            .priority(100),
        Node::row(vec![
            Node::pane(Pane::Preview)
                .width(Size::fill(30))
                .height(Size::fixed(8))
                .priority(20)
                .overflow(Overflow::Hide),
            Node::pane(Pane::Activity)
                .width(Size::fill(30))
                .height(Size::fixed(8))
                .overflow(Overflow::Hide),
        ])
        .wrap(Wrap::AtMinimum)
        .gap(1)
        .height(Size::bounded(0, 8, 16)),
    ])
    .gap(1)
}
struct App {
    focus: Focus<Pane>,
    quit: bool,
    area: Rect,
    theme: starkit::theme::Theme,
}
impl Default for App {
    fn default() -> Self {
        Self {
            focus: Focus::new(Some(Pane::Workspace)),
            quit: false,
            area: Rect::new(0, 0, 80, 24),
            theme: starkit::theme::Theme::resolve(
                &starkit::theme::ThemeFile::parse(starkit::theme::BUILTINS[2].toml).unwrap(),
            ),
        }
    }
}
impl App {
    fn key(&mut self, code: &str) {
        match code {
            "q" | "escape" => self.quit = true,
            "tab" => {
                self.focus.step(&tree().layout(self.area).unwrap(), true);
            }
            _ => {}
        }
    }
    fn draw(&mut self, area: Rect) -> Buffer {
        self.area = area;
        let layout = tree().layout(area).expect("bounded static tree");
        self.focus.reconcile(&layout);
        let mut buffer = Buffer::empty(area);
        for pane in &layout.panes {
            if let Some(rect) = pane.area {
                let title = match pane.id {
                    Pane::Workspace => "Workspace",
                    Pane::Preview => "Preview",
                    Pane::Activity => "Activity",
                };
                let body = frame::frame(
                    rect,
                    &mut buffer,
                    &Frame {
                        theme: &self.theme,
                        focused: self.focus.active() == Some(pane.id),
                        title,
                        detail: None,
                        heading: false,
                        badge: None,
                        footer: None,
                        words: NO_WORDS,
                    },
                );
                Paragraph::new("Tab changes focus. Resize to wrap panels. q exits.")
                    .style(Style::default())
                    .render(body, &mut buffer);
            }
        }
        buffer
    }
}
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    #[cfg(all(feature = "native", unix))]
    if matches!(
        args.first().map(String::as_str),
        Some("--native" | "--graphical-relay" | "--serve" | "--png" | "--ssh")
    ) {
        return native::run(&args);
    }
    let profile = Profile::resolve(&Environment::detect(), Observed::default());
    if args
        .first()
        .is_some_and(|arg| arg == "--inspect" || arg == "--replay")
    {
        let mut replay = Replay::new(80, 24, profile, Tier::Native, Some(Pane::Workspace))?;
        if args[0] == "--replay" {
            use std::io::Read;
            let mut input = String::new();
            std::io::stdin()
                .take(1_048_577)
                .read_to_string(&mut input)?;
            anyhow::ensure!(input.len() <= 1_048_576, "Replay input exceeds 1 MiB");
            let actions: Vec<Action<Pane>> = serde_json::from_str(&input)?;
            anyhow::ensure!(actions.len() <= 1024, "Replay exceeds 1024 actions");
            for action in actions {
                println!(
                    "{}",
                    serde_json::to_string(&replay.apply(&tree(), action)?)?
                );
            }
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&replay.snapshot(&tree())?)?
            );
        }
        return Ok(());
    }
    use starkit::crossterm::event::{self, Event, KeyCode, KeyEventKind};
    let mut terminal = starkit::term::Session::new()?;
    let mut app = App::default();
    while !app.quit {
        terminal.draw(|frame| {
            let mut buffer = app.draw(frame.area());
            profile.adapt_buffer(&mut buffer);
            *frame.buffer_mut() = buffer;
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Release {
                match key.code {
                    KeyCode::Tab => app.key("tab"),
                    KeyCode::Char('q') | KeyCode::Esc => app.key("q"),
                    _ => {}
                }
            }
        }
    }
    terminal.finish()?;
    Ok(())
}
