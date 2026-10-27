//! Independent public consumer: one application tree, cell and graphical views.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use starkit::{
    diagnostics::{Action, Replay},
    panes::{Node, Overflow, Size, Wrap},
    presentation::{Environment, Observed, Profile, Tier},
};
mod gallery;
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
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let theme = args
        .windows(2)
        .find(|pair| pair[0] == "--theme")
        .map(|pair| pair[1].as_str())
        .unwrap_or(starkit::theme::DEFAULT_ID);
    #[cfg(not(feature = "native"))]
    if args.first().is_some_and(|arg| {
        matches!(
            arg.as_str(),
            "--native" | "--png" | "--ssh" | "--serve" | "--graphical-relay"
        )
    }) {
        anyhow::bail!("This starter build has no graphical frontend; enable its native feature. The default cell UI remains available.");
    }
    #[cfg(feature = "native")]
    {
        if args.first().is_some_and(|arg| arg == "--png") {
            return gallery::png(
                theme,
                args.get(1).ok_or_else(|| anyhow::anyhow!("--png PATH"))?,
            );
        }
        if args.first().is_some_and(|arg| {
            matches!(
                arg.as_str(),
                "--native"
                    | "--serve"
                    | "--graphical-relay"
                    | "--ssh"
                    | "--attach"
                    | "--sessions"
                    | "--cells"
            )
        }) {
            return gallery::native(theme);
        }
    }
    if args.first().is_some_and(|arg| arg == "--gallery") {
        println!("{}", gallery::cell_text(theme, 100, 30)?);
        return Ok(());
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
    gallery::cells(theme)
}
