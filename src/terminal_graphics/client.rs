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
    fn spawn(pixels: bool, font: super::font::Font) -> Result<Self> {
        if pixels {
            Ok(Self::Pixels(Renderer::spawn_with_font(font)?))
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
    fn live_image(&mut self, id: String, pixels: std::sync::Arc<crate::image::RgbaImage>) {
        if let Self::Pixels(r) = self {
            r.live_image(id, pixels);
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
    child: Option<Child>,
    socket: Option<std::os::unix::net::UnixStream>,
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
        let stdin = child.stdin.take().context("session stdin unavailable")?;
        let stdout = child.stdout.take().context("session stdout unavailable")?;
        Self::streams(stdin, stdout, Some(child), None)
    }
    fn terminal_socket(path: &std::path::Path) -> Result<Self> {
        let mut socket = std::os::unix::net::UnixStream::connect(path)?;
        use std::io::Write;
        let nonce = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("Invalid terminal socket")?;
        writeln!(socket, "STAR_KIT_CLIENT {nonce}")?;
        Self::streams(socket.try_clone()?, socket.try_clone()?, None, Some(socket))
    }
    fn streams(
        mut stdin: impl io::Write + Send + 'static,
        stdout: impl io::Read + Send + 'static,
        child: Option<Child>,
        socket: Option<std::os::unix::net::UnixStream>,
    ) -> Result<Self> {
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
        let terminal = socket.is_some();
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
                    Ok(None) => {
                        if terminal {
                            let _ = control.send(ServerMessage::Closed);
                        }
                        break;
                    }
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
            socket,
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
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
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

fn scaled_viewport(mut viewport: Viewport, percent: u16) -> Viewport {
    viewport.columns = ((u32::from(viewport.columns) * 100 / u32::from(percent)) as u16)
        .max(viewport.columns.min(60));
    viewport.rows =
        ((u32::from(viewport.rows) * 100 / u32::from(percent)) as u16).max(viewport.rows.min(21));
    viewport
}

// Keep the original position through logical font scaling. Mapping via a
// rounded logical cell can move a click across a menu/action row boundary.
fn terminal_pixel(cell: u16, cells: u16, pixels: u32) -> u32 {
    (((u64::from(cell) * 2 + 1) * u64::from(pixels)) / (u64::from(cells.max(1)) * 2))
        .min(u64::from(pixels.saturating_sub(1))) as u32
}

fn logical_coordinate(value: u16, physical: u16, logical: u16) -> u16 {
    (u32::from(value) * u32::from(logical) / u32::from(physical.max(1))) as u16
}

fn logical_drop(
    text: &str,
    grid: (u16, u16),
    viewport: Viewport,
    placements: &[super::placement::Placement],
) -> String {
    let (header, payload) = text.split_once(';').unwrap_or((text, ""));
    if !header
        .split(':')
        .any(|field| matches!(field, "t=m" | "t=M" | "t=o"))
    {
        return text.into();
    }
    let coordinate = |axis: &str| {
        header.split(':').find_map(|f| {
            let (key, value) = f.split_once('=')?;
            (key == axis).then(|| value.parse::<u16>().ok()).flatten()
        })
    };
    let projected = coordinate("x").zip(coordinate("y")).map(|(x, y)| {
        if placements.is_empty() {
            (
                i32::from(logical_coordinate(x, grid.0, viewport.columns)),
                i32::from(logical_coordinate(y, grid.1, viewport.rows)),
            )
        } else if x >= grid.0 || y >= grid.1 {
            (-1, -1)
        } else {
            let pixel = coordinate("X")
                .zip(coordinate("Y"))
                .map(|(x, y)| [u32::from(x), u32::from(y)])
                .unwrap_or([
                    terminal_pixel(x, grid.0, viewport.width),
                    terminal_pixel(y, grid.1, viewport.height),
                ]);
            placements
                .iter()
                .rev()
                .find_map(|p| p.pointer_pixels(pixel[0], pixel[1], false))
                .map(|(x, y)| (i32::from(x), i32::from(y)))
                .unwrap_or((-1, -1))
        }
    });
    let header = header
        .split(':')
        .map(|field| {
            let Some((axis, value)) = field.split_once('=') else {
                return field.into();
            };
            let Ok(value) = value.parse::<u16>() else {
                return field.into();
            };
            match axis {
                "x" => format!("x={}", projected.map_or(i32::from(value), |p| p.0)),
                "y" => format!("y={}", projected.map_or(i32::from(value), |p| p.1)),
                _ => field.into(),
            }
        })
        .collect::<Vec<_>>()
        .join(":");
    if text.contains(';') {
        format!("{header};{payload}")
    } else {
        header
    }
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
    run_impl(launch, |_| None, false, None)
}

pub fn run_with_events(launch: Launch, custom: fn(&Event) -> Option<Input>) -> Result<()> {
    run_impl(launch, custom, true, None)
}

/// Frontend owned by a local terminal integration, attached through the existing SSH TTY.
pub fn run_terminal_socket_with_events(
    path: &std::path::Path,
    custom: fn(&Event) -> Option<Input>,
) -> Result<()> {
    let launch = Launch {
        executable: String::new(),
        host: None,
        ssh_config: None,
        session: "terminal".into(),
        directory: None,
        attach_only: true,
    };
    run_impl(launch, custom, true, Some(path))
}

fn run_impl(
    launch: Launch,
    custom: fn(&Event) -> Option<Input>,
    terminal_extensions: bool,
    terminal_socket: Option<&std::path::Path>,
) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("An interactive terminal is required to attach an application session");
    }
    let mut drop_bridge = super::drop_bridge::Bridge::new(
        (launch.host.is_some() || terminal_socket.is_some()) && terminal_extensions,
    );
    let mut font = super::font::probe().configured();
    let graphics = crate::graphics::Graphics::probe_if_tty(crate::graphics::Mode::Auto);
    let mut capabilities = super::capabilities::Capabilities::detected(&graphics);
    capabilities.local_media = launch.host.is_none() && terminal_socket.is_none();
    capabilities.video = capabilities.image_transport == super::capabilities::ImageTransport::Kitty;
    tracing::info!(?capabilities, "Terminal presentation capabilities");
    let pixels = capabilities.image_transport == super::capabilities::ImageTransport::Kitty;
    let cell = graphics.cell_size().unwrap_or((10, 20));
    font.cell = Some(cell);
    let mut size = viewport(1, cell)?;
    let mut terminal_grid = (size.columns, size.rows);
    let scale = if pixels {
        std::env::var("STAR_GRAPHICS_SCALE")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|n| (100..=200).contains(n))
            .unwrap_or(100)
    } else {
        100
    };
    size = scaled_viewport(size, scale);
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
    let mut renderer = Frontend::spawn(pixels, font)?;
    let mut media = super::media::Frontend::new(capabilities.local_media);
    let connect = || match terminal_socket {
        Some(path) => Connection::terminal_socket(path),
        None => Connection::spawn(&launch, control),
    };
    let mut connection = Some(connect()?);
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
    let mut pointer = super::pointer::Pointer::default();
    let mut resize_handles = Vec::new();
    let mut pointer_regions = Vec::new();
    let mut pointer_placements: Vec<super::placement::Placement> = Vec::new();
    let mut assets = std::collections::HashMap::<String, String>::new();
    let mut epoch: Option<String> = None;
    // Keep startup outside the pixel renderer. Its first capture must belong
    // to the application, rather than a competing placeholder scene.
    io::stdout().write_all(b"\x1b[HConnecting to application session...")?;
    io::stdout().flush()?;
    let mut last_scene: Option<Scene> = None;
    let mut shown: Option<(u64, u64)> = None;
    let mut id = 0u64;
    let mut connected = false;
    let mut last_reply = Instant::now();
    let mut ping = Instant::now();
    let mut last_input = Instant::now();
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
                ServerMessage::Media { message } => {
                    if let Some(c) = &connection {
                        media.receive(message, &c.input)?;
                    }
                }
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
                    let payload = drop_bridge.identity(&meta).map(str::to_owned).or(payload);
                    if !drop_bridge.request(&meta)? {
                        let mut out = io::stdout().lock();
                        write!(out, "\x1b]72;{meta}")?;
                        if let Some(payload) = payload {
                            write!(out, ";{payload}")?;
                        }
                        out.write_all(b"\x1b\\")?;
                        out.flush()?;
                    }
                    if let Some(c) = &connection {
                        c.send(ClientMessage::EffectAck { id: effect_id })?;
                    }
                }
                ServerMessage::Error { message } => {
                    fatal = Some(message);
                    break;
                }
                ServerMessage::Closed => {
                    pointer.reset(&mut io::stdout().lock())?;
                    resize_handles.clear();
                    pointer_regions.clear();
                    pointer_placements.clear();
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
        if let Some(c) = &connection {
            if let Some((id, pixels)) = media.tick(&c.input) {
                renderer.live_image(id, pixels);
            }
        }
        for message in renderer.output().try_iter().collect::<Vec<_>>() {
            match message {
                RenderMessage::Frame {
                    revision,
                    generation,
                    width,
                    height,
                    pixels: frame_pixels,
                } if generation == size.generation && width > 0 && height > 0 => {
                    let presentation_started = Instant::now();
                    if pixels {
                        bytes += presenter.present_pixels(
                            frame_pixels.context("Native frame has no pixels")?,
                            Viewport {
                                columns: terminal_grid.0,
                                rows: terminal_grid.1,
                                width,
                                height,
                                ..size
                            },
                            &mut io::stdout().lock(),
                        )? as u64;
                    }
                    frames += 1;
                    shown = Some((revision, generation));
                    if let Some(scene) = last_scene
                        .as_ref()
                        .filter(|s| connected && s.revision == revision)
                    {
                        resize_handles.clone_from(&scene.resize_handles);
                        pointer_regions.clone_from(&scene.pointer_regions);
                        pointer_placements.clone_from(&scene.placements);
                    }
                    tracing::debug!(
                        revision,
                        generation,
                        width,
                        height,
                        present_us = presentation_started.elapsed().as_micros(),
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
                    .and_then(|c| c.child.as_mut())
                    .map(|child| child.try_wait())
                    .transpose()?
                    .flatten()
                    .is_some());
        if dead {
            media.reset();
            if terminal_socket.is_some() {
                fatal = Some(
                    "Terminal bridge disconnected; remote operations continue. Relaunch to attach."
                        .into(),
                );
                break;
            }
            if epoch.is_none() {
                fatal = Some("Could not start graphical session. Check the feature-enabled host executable and its session log.".into());
                break;
            }
            drop_bridge.reset();
            io::stdout().write_all(b"\x1b]72;t=r:o=0:i=1\x1b\\")?;
            io::stdout().flush()?;
            connection = None;
            connected = false;
            pointer.reset(&mut io::stdout().lock())?;
            resize_handles.clear();
            pointer_regions.clear();
            pointer_placements.clear();
            shown = None;
            reconnect = Instant::now() + Duration::from_secs(5);
            if let Some(mut scene) = last_scene.clone() {
                scene.resize_handles.clear();
                scene.pointer_regions.clear();
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
        // Bounded file streaming leaves queue capacity for pointer/key input.
        // Disk reads and source cleanup never run on the presentation thread.
        if connected {
            if let Some(c) = &connection {
                for _ in 0..8 {
                    if c.input.len() >= 4 {
                        break;
                    }
                    let Ok(output) = drop_bridge.responses().try_recv() else {
                        break;
                    };
                    let current = drop_bridge.current(output.epoch);
                    match output.event {
                        super::drop_bridge::Event::Ready { client } if current => {
                            write!(io::stdout(), "\x1b]72;t=r:o=0:i={client}\x1b\\")?;
                            io::stdout().flush()?;
                            tracing::debug!("SSH drop captured; desktop pointer released");
                        }
                        super::drop_bridge::Event::Wire(text)
                            if current || text.starts_with("t=L:") =>
                        {
                            if text.starts_with("t=R:") {
                                io::stdout().write_all(b"\x1b]72;t=r:o=0:i=1\x1b\\")?;
                                io::stdout().flush()?;
                                drop_bridge.reset();
                            }
                            id += 1;
                            let (revision, generation) = shown.unwrap_or((0, size.generation));
                            c.send(ClientMessage::Input {
                                id,
                                revision,
                                generation,
                                input: Input::Osc72 { text },
                            })?;
                        }
                        _ => {}
                    }
                }
            }
        }
        // Renderer/socket readiness is independent of terminal input. During
        // interaction avoid stacking a whole idle poll onto each frame; return
        // to the quiet poll once the burst and its raster work have settled.
        let rendering = last_scene
            .as_ref()
            .is_some_and(|scene| shown != Some((scene.revision, scene.viewport.generation)));
        let poll_ms = if rendering || last_input.elapsed() < Duration::from_millis(100) {
            2
        } else {
            16
        };
        if event::poll(Duration::from_millis(poll_ms))? {
            last_input = Instant::now();
            let event = event::read()?;
            let mut input = custom(&event);
            match event {
                Event::Key(k) if k.kind == crate::crossterm::event::KeyEventKind::Press => {
                    pointer.reset(&mut io::stdout().lock())?;
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
                    let pixel = [
                        terminal_pixel(m.column, terminal_grid.0, size.width),
                        terminal_pixel(m.row, terminal_grid.1, size.height),
                    ];
                    let logical = if pointer_placements.is_empty() {
                        Some((
                            logical_coordinate(m.column, terminal_grid.0, size.columns),
                            logical_coordinate(m.row, terminal_grid.1, size.rows),
                        ))
                    } else {
                        pointer_placements
                            .iter()
                            .rev()
                            .find_map(|p| p.pointer_pixels(pixel[0], pixel[1], false))
                    };
                    let clickable = logical
                        .is_some_and(|(x, y)| pointer_regions.iter().any(|r| r.contains(x, y)));
                    pointer.update(
                        action,
                        button == 0,
                        pixel,
                        &resize_handles,
                        clickable,
                        &mut io::stdout().lock(),
                    )?;
                    input = Some(Input::Pointer {
                        pixel: Some(pixel),
                        action: action.into(),
                        button,
                        x: logical_coordinate(m.column, terminal_grid.0, size.columns),
                        y: logical_coordinate(m.row, terminal_grid.1, size.rows),
                        modifiers: m.modifiers.bits(),
                    });
                }
                Event::Paste(text) => input = Some(Input::Paste { text }),
                Event::FocusLost => {
                    pointer.reset(&mut io::stdout().lock())?;
                    input = Some(Input::CancelPointer);
                }
                Event::Resize(..) => {
                    pointer.reset(&mut io::stdout().lock())?;
                    resize_handles.clear();
                    pointer_regions.clear();
                    pointer_placements.clear();
                    size = viewport(size.generation + 1, cell)?;
                    terminal_grid = (size.columns, size.rows);
                    size = scaled_viewport(size, scale);
                    shown = None;
                    input = Some(Input::Resize { viewport: size });
                }
                _ => {}
            }
            if let Some(Input::Osc72 { text }) = input.as_mut() {
                match drop_bridge.terminal(text) {
                    Ok(false) => input = None,
                    Ok(true) => {
                        *text = logical_drop(
                            text,
                            terminal_grid,
                            size,
                            last_scene.as_ref().map_or(&[], |s| s.placements.as_slice()),
                        );
                    }
                    Err(error) => {
                        io::stdout().write_all(b"\x1b]72;t=r:o=0:i=1\x1b\\")?;
                        io::stdout().flush()?;
                        drop_bridge.reset();
                        *text = format!("t=R:i=1;EIO:{error}");
                    }
                }
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
    drop_bridge.reset();
    io::stdout().write_all(b"\x1b]72;t=r:o=0:i=1\x1b\\")?;
    io::stdout().flush()?;
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
    fn popup_hits_keep_original_terminal_centres_at_every_font_scale() {
        use super::super::placement::Placement;
        use crate::native_surface::PixelRect;
        let physical = Viewport {
            columns: 251,
            rows: 60,
            width: 1757,
            height: 960,
            generation: 1,
        };
        let placement = Placement::new(
            Rect {
                x: 50,
                y: 20,
                width: 70,
                height: 6,
            },
            PixelRect::new(500, 475, 560, 108),
        );
        let mut rounding_misses = 0;
        for scale in [100, 115, 125, 150, 200] {
            let scaled = scaled_viewport(physical, scale);
            for row in 0..physical.rows {
                let px = terminal_pixel(100, physical.columns, physical.width);
                let py = terminal_pixel(row, physical.rows, physical.height);
                let expected = if (475..583).contains(&py) {
                    Some((50 + ((px - 500) / 8) as u16, 20 + ((py - 475) / 18) as u16))
                } else {
                    None
                };
                assert_eq!(
                    placement.pointer_pixels(px, py, false),
                    expected,
                    "scale={scale} row={row}"
                );
                if placement.pointer(
                    scaled,
                    logical_coordinate(100, physical.columns, scaled.columns),
                    logical_coordinate(row, physical.rows, scaled.rows),
                    false,
                ) != expected
                {
                    rounding_misses += 1;
                }
            }
        }
        assert!(
            rounding_misses > 0,
            "fixture must exercise the original double-rounding bug"
        );
    }

    #[test]
    fn desktop_drag_uses_pixel_pane_geometry_and_rejects_gutters() {
        use super::super::placement::Placement;
        use crate::native_surface::PixelRect;
        let v = Viewport {
            columns: 100,
            rows: 40,
            width: 1000,
            height: 800,
            generation: 1,
        };
        let p = Placement::new(
            Rect {
                x: 50,
                y: 3,
                width: 50,
                height: 20,
            },
            PixelRect::new(510, 70, 480, 400),
        );
        let expected = p.pointer_pixels(625, 175, false).unwrap();
        for ty in ["o", "m", "M"] {
            let raw = format!("t={ty}:x=62:y=8:X=625:Y=175:o=1:i=1;text/uri-list");
            let mapped = logical_drop(&raw, (100, 40), v, std::slice::from_ref(&p));
            assert!(mapped.contains(&format!(":x={}:y={}:X=625:Y=175:", expected.0, expected.1)));
        }
        assert_eq!(
            logical_drop("t=m:x=50:y=8:o=1;i", (100, 40), v, &[p]),
            "t=m:x=-1:y=-1:o=1;i"
        );
    }

    #[test]
    fn scaled_graphics_keep_mouse_and_desktop_drop_targets_aligned() {
        let viewport = scaled_viewport(
            Viewport {
                columns: 240,
                rows: 120,
                ..Viewport::default()
            },
            150,
        );
        assert_eq!((viewport.columns, viewport.rows), (160, 80));
        assert_eq!(logical_coordinate(120, 240, viewport.columns), 80);
        assert_eq!(logical_coordinate(119, 120, viewport.rows), 79);
        assert_eq!(
            logical_coordinate(240, 240, viewport.columns),
            160,
            "outside remains outside"
        );
        assert_eq!(
            logical_drop(
                "t=M:x=120:y=60:o=1:i=1;text/uri-list",
                (240, 120),
                viewport,
                &[]
            ),
            "t=M:x=80:y=40:o=1:i=1;text/uri-list"
        );
        assert_eq!(
            logical_drop("t=r:x=1:y=2;i=1", (240, 120), viewport, &[]),
            "t=r:x=1:y=2;i=1"
        );
        assert_eq!(
            logical_drop("t=m:x=-1:y=-1:i=1", (240, 120), viewport, &[]),
            "t=m:x=-1:y=-1:i=1"
        );
        let minimum = scaled_viewport(
            Viewport {
                columns: 60,
                rows: 21,
                ..Viewport::default()
            },
            150,
        );
        assert_eq!((minimum.columns, minimum.rows), (60, 21));
    }
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
