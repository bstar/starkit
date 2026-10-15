//! Presentation-only client: runs on Linux/macOS, with no application backend.
use anyhow::{bail, Context, Result};
use starkit::terminal_graphics::client::{self, Launch};
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let paths = starkit::paths::Paths::new(
        "starkitfrontend",
        "STARKITFRONTEND_DIR",
        "STARKITFRONTEND_CONFIG_DIR",
    );
    let _logging = if args.iter().any(|a| a == "--help" || a == "-h") {
        None
    } else {
        Some(starkit::logging::init(&paths, false)?)
    };
    if args.first().is_some_and(|a| a == "--terminal-frontend") {
        return client::run_terminal_socket_with_events(
            std::path::Path::new(args.get(1).context("--terminal-frontend SOCKET")?),
            |_| None,
        );
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("STAR/KIT session frontend\n\nstar-kit-frontend --ssh HOST --project REMOTE_DIRECTORY [--session NAME] [--executable starreel] [--ssh-config FILE]\n\nRuns video presentation and audio output locally; recording and apps stay on the host.\nThe presentation terminal must support Kitty graphics. Build with terminal-graphics,media for H.264 live preview and audio. The remote executable must support streamed live canvases.");
        return Ok(());
    }
    let mut host = None;
    let mut directory = None;
    let mut session = "studio".to_string();
    let mut executable = "starreel".to_string();
    let mut config = None;
    let mut attach = false;
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--attach-only" {
            attach = true;
            i += 1;
            continue;
        }
        let value = args
            .get(i + 1)
            .with_context(|| format!("Missing value for {key}"))?
            .clone();
        match key.as_str() {
            "--ssh" => host = Some(value),
            "--project" | "--directory" => directory = Some(value),
            "--session" => session = value,
            "--executable" => executable = value,
            "--ssh-config" => config = Some(value.into()),
            _ => bail!("Unknown option {key}"),
        }
        i += 2;
    }
    anyhow::ensure!(host.is_some(), "Use --ssh HOST (or --help)");
    client::run(Launch {
        executable,
        host,
        ssh_config: config,
        session,
        directory,
        attach_only: attach,
        play: None,
    })
}
