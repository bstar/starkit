//! Local Kitty presentation and disposable stdio/SSH attachments.
use std::io::{self, BufReader, IsTerminal as _};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};

use super::protocol::*;
use super::renderer::{KittyPresenter, RenderMessage, Renderer};
use crate::crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};

enum Frontend {
    Pixels(Renderer),
    Cells(super::cells::Cells),
}
impl Frontend {
    fn spawn(pixels: bool) -> Result<Self> {
        if pixels {
            Ok(Self::Pixels(Renderer::spawn()?))
        } else {
            Ok(Self::Cells(super::cells::Cells::default()))
        }
    }
    fn scene(&mut self, scene: &Scene) -> Result<()> {
        match self {
            Self::Pixels(r) => r.scene(scene),
            Self::Cells(r) => r.scene(scene),
        }
    }
    fn clipboard(&mut self, text: &str) -> Result<()> {
        match self {
            Self::Pixels(r) => r.clipboard(text),
            Self::Cells(r) => r.clipboard(text),
        }
    }
    fn output(&self) -> &Receiver<RenderMessage> {
        match self {
            Self::Pixels(r) => &r.output,
            Self::Cells(r) => r.output(),
        }
    }
    fn alive(&mut self) -> Result<bool> {
        match self {
            Self::Pixels(r) => r.alive(),
            Self::Cells(_) => Ok(true),
        }
    }
}

/// The remote command is assembled only from quoted arguments, never user code.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Clone)]
pub struct Launch {
    pub executable: String,
    pub host: Option<String>,
    pub ssh_config: Option<std::path::PathBuf>,
    pub session: String,
    pub directory: Option<String>,
    pub attach_only: bool,
}
impl Launch {
    fn command(&self, control: Option<&std::path::Path>) -> Result<Command> {
        let mut args = vec!["--graphical-relay".to_string(), self.session.clone()];
        if self.attach_only {
            args.push("--attach-only".into());
        }
        if let Some(dir) = &self.directory {
            args.extend(["--directory".into(), dir.clone()]);
        }
        if let Some(host) = &self.host {
            if host.starts_with('-') || host.contains('\0') {
                bail!("Invalid SSH host");
            }
            let remote = std::iter::once(self.executable.as_str())
                .chain(args.iter().map(String::as_str))
                .map(shell_quote)
                .collect::<Vec<_>>()
                .join(" ");
            let mut command = Command::new("ssh");
            if let Some(control) = control {
                command.arg("-S").arg(control).args(["-o", "BatchMode=yes"]);
            }
            if let Some(config) = &self.ssh_config {
                command.arg("-F").arg(config);
            }
            command.args([
                "-T",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "--",
                host,
                &remote,
            ]);
            Ok(command)
        } else {
            let mut command = Command::new(&self.executable);
            command.args(args);
            Ok(command)
        }
    }
}
struct Connection {
    child: Child,
    input: Sender<ClientMessage>,
    messages: Receiver<ServerMessage>,
    frames: Receiver<Scene>,
}
impl Connection {
    fn spawn(launch: &Launch, control: Option<&std::path::Path>) -> Result<Self> {
        let mut child = launch
            .command(control)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut stdin = child.stdin.take().context("session stdin unavailable")?;
        let stdout = child.stdout.take().context("session stdout unavailable")?;
        let (input, rx) = bounded(64);
        std::thread::spawn(move || {
            for message in rx {
                if write_message(&message, &mut stdin).is_err() {
                    return;
                }
            }
        });
        let (control, messages) = bounded(64);
        let (scene_tx, frames) = bounded(1);
        let old = frames.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message::<ServerMessage>(&mut reader) {
                    Ok(Some(ServerMessage::Scene { scene })) => {
                        if scene.viewport.validate().is_err() {
                            let _ = control.send(ServerMessage::Error {
                                message: "Invalid remote viewport".into(),
                            });
                            break;
                        }
                        if let Err(crossbeam_channel::TrySendError::Full(scene)) =
                            scene_tx.try_send(scene)
                        {
                            let _ = old.try_recv();
                            let _ = scene_tx.try_send(scene);
                        }
                    }
                    Ok(Some(message)) => {
                        if control.send(message).is_err() {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = control.send(ServerMessage::Error {
                            message: error.to_string(),
                        });
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            messages,
            frames,
        })
    }
    fn send(&self, message: ClientMessage) -> Result<()> {
        self.input.try_send(message).map_err(|_| {
            anyhow::anyhow!("Session input queue is unavailable; reconnect before continuing")
        })
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crate::term::restore();
    }
}
struct PresenterGuard(KittyPresenter);
impl std::ops::Deref for PresenterGuard {
    type Target = KittyPresenter;
    fn deref(&self) -> &KittyPresenter {
        &self.0
    }
}
impl std::ops::DerefMut for PresenterGuard {
    fn deref_mut(&mut self) -> &mut KittyPresenter {
        &mut self.0
    }
}
impl Drop for PresenterGuard {
    fn drop(&mut self) {
        let _ = self.0.clear(&mut io::stdout().lock());
    }
}

struct SshGuard {
    directory: std::path::PathBuf,
    socket: std::path::PathBuf,
    host: String,
}
impl Drop for SshGuard {
    fn drop(&mut self) {
        let _ = Command::new("ssh")
            .arg("-S")
            .arg(&self.socket)
            .args(["-O", "exit", "--", &self.host])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn authenticate(launch: &Launch) -> Result<Option<SshGuard>> {
    let Some(host) = &launch.host else {
        return Ok(None);
    };
    // Validate before passing a hostname to SSH option parsing.
    launch.command(None)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory =
        std::path::PathBuf::from("/tmp").join(format!("star-ssh-{}-{nonce}", std::process::id()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&directory)?;
    let socket = directory.join("s");
    let mut command = Command::new("ssh");
    if let Some(config) = &launch.ssh_config {
        command.arg("-F").arg(config);
    }
    let status = command
        .args([
            "-T",
            "-o",
            "ControlMaster=yes",
            "-o",
            "ControlPersist=120",
            "-o",
        ])
        .arg(format!("ControlPath={}", socket.display()))
        .args(["--", host, "true"])
        .stdout(Stdio::null())
        .status()?;
    let guard = SshGuard {
        directory,
        socket,
        host: host.clone(),
    };
    if !status.success() {
        bail!("SSH authentication failed; terminal graphics have not taken over the keyboard");
    }
    Ok(Some(guard))
}

fn viewport(generation: u64, cell: (u16, u16)) -> Result<Viewport> {
    let (columns, rows) = crate::crossterm::terminal::size()?;
    let size = crate::crossterm::terminal::window_size().ok();
    let width = size
        .as_ref()
        .map(|s| u32::from(s.width))
        .filter(|n| *n > 0)
        .unwrap_or(u32::from(columns) * u32::from(cell.0));
    let height = size
        .as_ref()
        .map(|s| u32::from(s.height))
        .filter(|n| *n > 0)
        .unwrap_or(u32::from(rows) * u32::from(cell.1));
    Ok(Viewport {
        columns,
        rows,
        width,
        height,
        generation,
    }
    .validate()?)
}

pub fn key(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => return format!("char:{c}"),
        KeyCode::F(n) => return format!("f:{n}"),
        KeyCode::Enter => "enter",
        KeyCode::Esc => "escape",
        KeyCode::Tab => "tab",
        KeyCode::BackTab => "backtab",
        KeyCode::Backspace => "backspace",
        KeyCode::Delete => "delete",
        KeyCode::Insert => "insert",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        _ => "unknown",
    }
    .to_string()
}

pub fn run(launch: Launch) -> Result<()> {
    run_impl(launch, |_| None, false)
}

pub fn run_with_events(launch: Launch, custom: fn(&Event) -> Option<Input>) -> Result<()> {
    run_impl(launch, custom, true)
}

fn run_impl(
    launch: Launch,
    custom: fn(&Event) -> Option<Input>,
    terminal_extensions: bool,
) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("An interactive terminal is required to attach an application session");
    }
    let graphics = crate::graphics::Graphics::probe_if_tty(crate::graphics::Mode::Auto);
    let capabilities = super::capabilities::Capabilities::detected(&graphics);
    tracing::info!(?capabilities, "Terminal presentation capabilities");
    let pixels = capabilities.image_transport == super::capabilities::ImageTransport::Kitty;
    let cell = graphics.cell_size().unwrap_or((10, 20));
    let mut size = viewport(1, cell)?;
    // SSH may need a password, key passphrase or host-key confirmation. Finish
    // that interaction before raw mode; the relay then reuses this connection.
    let ssh = authenticate(&launch)?;
    let control = ssh.as_ref().map(|s| s.socket.as_path());
    if pixels {
        eprintln!("Starting STAR graphical renderer in this Kitty terminal…");
    } else {
        eprintln!("Graphics unavailable; using the terminal interface for this session.");
    }
    let started = Instant::now();
    let mut renderer = Frontend::spawn(pixels)?;
    let mut connection = Some(Connection::spawn(&launch, control)?);
    let client = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    connection
        .as_ref()
        .expect("connected")
        .send(ClientMessage::Hello {
            capabilities: Some(capabilities),
            version: VERSION,
            viewport: size,
            client: client.clone(),
        })?;
    let _terminal = crate::term::init()?;
    let _guard = TerminalGuard;
    use std::io::Write;
    let mut presenter = PresenterGuard(KittyPresenter::default());
    let mut assets = std::collections::HashMap::<String, String>::new();
    let mut epoch: Option<String> = None;
    let loading = Scene {
        revision: 0,
        interaction: 0,
        viewport: size,
        background: "#1e1e2e".into(),
        foreground: "#cdd6f4".into(),
        accent: "#89b4fa".into(),
        border: "#45475a".into(),
        spans: vec![Span {
            x: 3,
            y: 3,
            text: "Connecting to application session…".into(),
            foreground: "#cdd6f4".into(),
            background: "#1e1e2e".into(),
            bold: true,
        }],
        components: vec![Component::Panel {
            rect: Rect {
                x: 1,
                y: 1,
                width: size.columns.saturating_sub(2),
                height: size.rows.saturating_sub(2),
            },
            active: true,
        }],
    };
    renderer.scene(&loading)?;
    let mut last_scene: Option<Scene> = Some(loading);
    let mut shown: Option<(u64, u64)> = None;
    let mut id = 0u64;
    let mut connected = false;
    let mut last_reply = Instant::now();
    let mut ping = Instant::now();
    let mut reconnect = Instant::now() + Duration::from_secs(5);
    let mut fatal = None;
    let mut frames = 0u64;
    let mut bytes = 0u64;
    loop {
        let controls = connection
            .as_ref()
            .map(|c| c.messages.try_iter().take(64).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut closed = false;
        for message in controls {
            last_reply = Instant::now();
            match message {
                ServerMessage::Hello {
                    version, epoch: e, ..
                } => {
                    if version != VERSION {
                        fatal = Some("Remote protocol version does not match".to_string());
                        break;
                    }
                    if epoch.as_ref().is_some_and(|old| old != &e) {
                        assets.clear();
                        tracing::warn!(
                            "Remote session restarted; previous commands will not be replayed"
                        );
                    }
                    epoch = Some(e);
                    tracing::debug!("Graphical session handshake received");
                    connected = true;
                    last_scene = None;
                    // Geometry may change while the handshake is pending.
                    // Always request a fresh scene before enabling user input.
                    id += 1;
                    if let Some(c) = &connection {
                        c.send(ClientMessage::Input {
                            id,
                            revision: 0,
                            generation: size.generation,
                            input: Input::Resize { viewport: size },
                        })?;
                    }
                    // OSC 72 is an application extension, not part of the
                    // graphical transport. Only an extension-aware input
                    // handler can safely consume its terminal replies.
                    if terminal_extensions {
                        io::stdout().write_all(b"\x1b]72;t=q:i=1\x1b\\\x1b[c")?;
                        io::stdout().flush()?;
                    }
                    shown = None;
                }
                ServerMessage::Clipboard { text } => renderer.clipboard(&text)?,
                ServerMessage::Asset { id, png } => {
                    super::assets::validate_png(&png)?;
                    if assets.len() > 8 {
                        assets.clear();
                    }
                    assets.insert(id.clone(), png.clone());
                    // Control and latest-scene queues are independent. An asset
                    // can arrive just after the scene that references it.
                    if let Some(scene) = &mut last_scene {
                        let mut changed = false;
                        for component in &mut scene.components {
                            if let Component::Image {
                                id: image_id,
                                png: image,
                                ..
                            } = component
                            {
                                if *image_id == id {
                                    *image = Some(png.clone());
                                    changed = true;
                                }
                            }
                        }
                        if changed {
                            renderer.scene(scene)?;
                        }
                    }
                }
                ServerMessage::Osc72 {
                    id: effect_id,
                    meta,
                    payload,
                } => {
                    if meta.len() > 4096
                        || payload.as_ref().is_some_and(|s| s.len() > MAX_MESSAGE / 2)
                        || meta
                            .bytes()
                            .chain(payload.as_deref().unwrap_or("").bytes())
                            .any(|b| b == 0x1b || b == 7)
                    {
                        fatal = Some("Invalid drag/drop message".into());
                        break;
                    }
                    let mut out = io::stdout().lock();
                    write!(out, "\x1b]72;{meta}")?;
                    if let Some(payload) = payload {
                        write!(out, ";{payload}")?;
                    }
                    out.write_all(b"\x1b\\")?;
                    out.flush()?;
                    if let Some(c) = &connection {
                        c.send(ClientMessage::EffectAck { id: effect_id })?;
                    }
                }
                ServerMessage::Error { message } => {
                    fatal = Some(message);
                    break;
                }
                ServerMessage::Closed => {
                    closed = true;
                    break;
                }
                _ => {}
            }
        }
        if fatal.is_some() || closed {
            break;
        }
        if let Some(mut scene) = connection.as_ref().and_then(|c| c.frames.try_iter().last()) {
            tracing::debug!(
                revision = scene.revision,
                generation = scene.viewport.generation,
                "Graphical scene received"
            );
            if scene.viewport.generation == size.generation {
                for component in &mut scene.components {
                    if let Component::Image { id, png, .. } = component {
                        *png = assets.get(id).cloned();
                    }
                }
                renderer.scene(&scene)?;
                last_scene = Some(scene);
            }
        }
        for message in renderer.output().try_iter().collect::<Vec<_>>() {
            match message {
                RenderMessage::Frame {
                    revision,
                    generation,
                    width,
                    height,
                    png,
                } if generation == size.generation && width > 0 && height > 0 => {
                    if pixels {
                        bytes += presenter.present_regions(
                            &png,
                            Viewport {
                                width,
                                height,
                                ..size
                            },
                            &mut io::stdout().lock(),
                        )? as u64;
                    }
                    frames += 1;
                    shown = Some((revision, generation));
                    tracing::debug!(
                        revision,
                        generation,
                        width,
                        height,
                        "Graphical frame presented"
                    );
                    if connected {
                        if let Some(c) = &connection {
                            c.send(ClientMessage::Presented {
                                revision,
                                generation,
                            })?;
                        }
                    }
                }
                RenderMessage::Error { message } => {
                    fatal = Some(message);
                    break;
                }
                _ => {}
            }
        }
        if !renderer.alive()? {
            fatal = Some("Graphical renderer stopped".into());
            break;
        }
        if connected && ping.elapsed() >= Duration::from_secs(2) {
            if let Some(c) = &connection {
                let _ = c.send(ClientMessage::Ping);
            }
            ping = Instant::now();
        }
        let timed_out = last_reply.elapsed() > Duration::from_secs(if connected { 10 } else { 20 });
        let dead = connection.is_some()
            && (timed_out
                || connection
                    .as_mut()
                    .map(|c| c.child.try_wait())
                    .transpose()?
                    .flatten()
                    .is_some());
        if dead {
            if epoch.is_none() {
                fatal = Some("Could not start graphical session. Check the feature-enabled host executable and its session log.".into());
                break;
            }
            connection = None;
            connected = false;
            shown = None;
            reconnect = Instant::now() + Duration::from_secs(5);
            if let Some(mut scene) = last_scene.clone() {
                scene.spans.push(Span{x:1,y:size.rows.saturating_sub(1),text:"Disconnected · remote operations continue · Ctrl+R reconnect · Ctrl+Q detach".into(),foreground:"#f38ba8".into(),background:scene.background.clone(),bold:true});
                renderer.scene(&scene)?;
            }
        }
        if connection.is_none() && Instant::now() >= reconnect {
            // Reattachment cannot recreate a vanished session implicitly.
            let mut attach = launch.clone();
            attach.attach_only = true;
            attach.directory = None;
            size.generation += 1;
            match Connection::spawn(&attach, control) {
                Ok(c) => {
                    c.send(ClientMessage::Hello {
                        capabilities: Some(capabilities),
                        version: VERSION,
                        viewport: size,
                        client: client.clone(),
                    })?;
                    connection = Some(c);
                    last_reply = Instant::now();
                }
                Err(error) => tracing::warn!(%error,"session reconnect failed"),
            }
            reconnect = Instant::now() + Duration::from_secs(5);
        }
        if event::poll(Duration::from_millis(16))? {
            let event = event::read()?;
            let mut input = custom(&event);
            match event {
                Event::Key(k) if k.kind == crate::crossterm::event::KeyEventKind::Press => {
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('q') {
                        break;
                    }
                    if !connected
                        && k.modifiers.contains(KeyModifiers::CONTROL)
                        && k.code == KeyCode::Char('r')
                    {
                        reconnect = Instant::now();
                    } else {
                        input = Some(Input::Key {
                            code: key(k.code),
                            modifiers: k.modifiers.bits(),
                        });
                    }
                }
                Event::Mouse(m) => {
                    let (action, button) = match m.kind {
                        MouseEventKind::Down(b) => ("down", Some(b)),
                        MouseEventKind::Up(b) => ("up", Some(b)),
                        MouseEventKind::Drag(b) => ("drag", Some(b)),
                        MouseEventKind::Moved => ("move", None),
                        MouseEventKind::ScrollUp => ("scroll_up", None),
                        MouseEventKind::ScrollDown => ("scroll_down", None),
                        MouseEventKind::ScrollLeft => ("scroll_left", None),
                        MouseEventKind::ScrollRight => ("scroll_right", None),
                    };
                    let button = match button {
                        Some(MouseButton::Right) => 1,
                        Some(MouseButton::Middle) => 2,
                        _ => 0,
                    };
                    input = Some(Input::Pointer {
                        action: action.into(),
                        button,
                        x: m.column,
                        y: m.row,
                        modifiers: m.modifiers.bits(),
                    });
                }
                Event::Paste(text) => input = Some(Input::Paste { text }),
                Event::Resize(..) => {
                    size = viewport(size.generation + 1, cell)?;
                    shown = None;
                    input = Some(Input::Resize { viewport: size });
                }
                _ => {}
            }
            let ready = last_scene.as_ref().is_some_and(|s| s.revision > 0)
                && shown.is_some_and(|(revision, generation)| {
                    revision > 0 && generation == size.generation
                });
            if connected && (ready || input.as_ref().is_some_and(control_input)) {
                if let (Some(c), Some(input)) = (&connection, input) {
                    id += 1;
                    let (revision, generation) = shown.unwrap_or((0, size.generation));
                    c.send(ClientMessage::Input {
                        id,
                        revision,
                        generation,
                        input,
                    })?;
                }
            }
        }
    }
    if let Some(c) = &connection {
        id += 1;
        let _ = c.send(ClientMessage::Input {
            id,
            revision: 0,
            generation: size.generation,
            input: Input::Detach,
        });
    }
    presenter.clear(&mut io::stdout().lock())?;
    tracing::info!(
        frames,
        bytes,
        elapsed_ms = started.elapsed().as_millis(),
        "terminal graphical presentation ended"
    );
    if let Some(message) = fatal {
        bail!("Graphical session: {message}");
    }
    Ok(())
}

// Geometry and capability negotiation must continue while waiting for a frame.
// File actions still require an authoritative scene and its displayed geometry.
fn control_input(input: &Input) -> bool {
    match input {
        Input::Resize { .. } => true,
        Input::Osc72 { text } => text
            .split(';')
            .next()
            .unwrap_or("")
            .split(':')
            .any(|field| matches!(field, "t=q" | "t=a")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_geometry_and_capabilities_bypass_the_first_frame_guard() {
        assert!(control_input(&Input::Resize {
            viewport: Viewport {
                columns: 80,
                rows: 24,
                width: 800,
                height: 480,
                generation: 2,
            }
        }));
        assert!(control_input(&Input::Osc72 {
            text: "t=q;".into()
        }));
        assert!(!control_input(&Input::Osc72 {
            text: "t=M;file".into()
        }));
        assert!(!control_input(&Input::Paste {
            text: "file".into()
        }));
    }
    #[test]
    fn remote_paths_are_literal_shell_arguments() {
        assert_eq!(
            shell_quote("a'$(touch /tmp/no)"),
            "'a'\\''$(touch /tmp/no)'"
        );
        assert!(Launch {
            executable: "starfold".into(),
            host: Some("-ProxyCommand=bad".into()),
            ssh_config: None,
            session: "test".into(),
            directory: None,
            attach_only: false
        }
        .command(None)
        .is_err());
    }
}
