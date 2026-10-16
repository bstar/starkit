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
fn stopped_renderer_message(output: &Receiver<RenderMessage>) -> String {
    output
        .try_iter()
        .find_map(|message| match message {
            RenderMessage::Error { message } => Some(message),
            _ => None,
        })
        .unwrap_or_else(|| "Graphical renderer stopped".into())
}
impl Frontend {
    fn spawn(pixels: bool, font: super::font::Font, options: PresentationOptions) -> Result<Self> {
        if pixels {
            Ok(Self::Pixels(Renderer::spawn_with_options(font, options)?))
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

#[allow(clippy::too_many_arguments)]
fn present_video(
    layer: &mut super::video_presenter::VideoPresenter,
    cells: &mut super::cell_video::CellVideo,
    pixels: bool,
    transport: super::capabilities::ImageTransport,
    force: bool,
    scene: &Scene,
    viewport: Viewport,
    out: &mut impl io::Write,
) -> Result<()> {
    if pixels || transport == super::capabilities::ImageTransport::Kitty {
        layer.set_cell_mode(!pixels, out)?;
        if !pixels {
            if let Some((id, _)) = &layer.frame {
                cells.clear_live(scene, id, force, out)?;
            }
        }
        layer.present(scene, viewport, out)?;
    } else if let Some((id, frame)) = &layer.frame {
        cells.present(scene, id, frame, force, out)?;
    }
    Ok(())
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
    pub play: Option<String>,
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
    outbox: std::sync::Arc<super::input_queue::Outbox>,
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
        let (control, messages) = bounded(64);
        let (outbox, wake) = super::input_queue::Outbox::new();
        let outbox = std::sync::Arc::new(outbox);
        let writer = outbox.clone();
        let errors = control.clone();
        std::thread::spawn(move || {
            let result = (|| -> io::Result<()> {
                loop {
                    if writer.closed() {
                        return Ok(());
                    }
                    // UI positions coalesce independently of pipe/socket backpressure.
                    // Audio/media traffic retains its existing bounded channel.
                    for message in rx.try_iter().take(8) {
                        write_message(&message, &mut stdin)?;
                    }
                    for _ in 0..16 {
                        let Some(message) = writer.pop() else {
                            break;
                        };
                        write_message(&message, &mut stdin)?;
                    }
                    if writer.closed() {
                        return Ok(());
                    }
                    if writer.pending() {
                        continue;
                    }
                    crossbeam_channel::select! {
                        recv(rx)->message=>match message{Ok(message)=>write_message(&message,&mut stdin)?,Err(_)=>return Ok(())},
                        recv(wake)->_=>{},
                    }
                }
            })();
            writer.close();
            if let Err(error) = result {
                let _ = errors.try_send(ServerMessage::Error {
                    message: format!("Session input connection closed: {error}"),
                });
            }
        });
        let (scene_tx, frames) = bounded(1);
        let old = frames.clone();
        let terminal = socket.is_some();
        // PCM reception and refill must not wait for terminal writes or rendering.
        #[cfg(feature = "audio-output")]
        let (pcm_tx, pcm_rx) = bounded(8);
        #[cfg(feature = "audio-output")]
        let pcm_out = input.clone();
        #[cfg(feature = "audio-output")]
        let pcm_errors = control.clone();
        #[cfg(feature = "audio-output")]
        std::thread::spawn(move || {
            let mut pcm = super::audio_v2::Frontend::default();
            loop {
                match pcm_rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(message) => {
                        if let Err(error) = pcm.receive(&message) {
                            let _ = pcm_errors.try_send(ServerMessage::Error {
                                message: format!("Local audio: {error:#}"),
                            });
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                pcm.tick(&pcm_out);
            }
            pcm.reset();
        });

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
                    #[cfg(feature = "audio-output")]
                    Ok(Some(ServerMessage::Media { message }))
                        if matches!(
                            message,
                            super::media::ToClient::PcmPause { .. }
                                | super::media::ToClient::PcmOpen { .. }
                                | super::media::ToClient::PcmCache { .. }
                                | super::media::ToClient::PcmChunk { .. }
                                | super::media::ToClient::PcmEnd { .. }
                                | super::media::ToClient::PcmClose { .. }
                        ) =>
                    {
                        if pcm_tx.send(message).is_err() {
                            return;
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
            outbox,
            messages,
            frames,
        })
    }
    fn send(&self, message: ClientMessage) -> Result<()> {
        self.outbox.push(message)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.outbox.close();
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
    // Read cells and pixels from one winsize snapshot. Two separate queries
    // can straddle a resize and describe a canvas wider than the actual window.
    let size = crate::crossterm::terminal::window_size().ok();
    let (columns, rows) = match &size {
        Some(size) if size.columns > 0 && size.rows > 0 => (size.columns, size.rows),
        _ => crate::crossterm::terminal::size()?,
    };
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
        KeyCode::CapsLock => "caps_lock",
        KeyCode::NumLock => "num_lock",
        KeyCode::Modifier(modifier) => {
            use crate::crossterm::event::ModifierKeyCode::*;
            match modifier {
                LeftShift => "shift_left",
                RightShift => "shift_right",
                LeftControl => "control_left",
                RightControl => "control_right",
                LeftAlt => "alt_left",
                RightAlt => "alt_right",
                LeftSuper => "super_left",
                RightSuper => "super_right",
                _ => "unknown",
            }
        }
        _ => "unknown",
    }
    .to_string()
}

pub fn run(launch: Launch) -> Result<()> {
    run_impl(
        launch,
        |_| None,
        false,
        None,
        || None,
        PresentationOptions::default(),
        None,
        None,
    )
}

pub fn run_with_events(launch: Launch, custom: fn(&Event) -> Option<Input>) -> Result<()> {
    run_impl(
        launch,
        custom,
        true,
        None,
        || None,
        PresentationOptions::default(),
        None,
        None,
    )
}

/// Local display preferences, independent of the remote application session.
#[derive(Debug, Clone, Copy)]
pub struct PresentationOptions {
    /// Pane border radius in display pixels. Zero uses square corners.
    pub pane_corner_radius: u16,
    /// Video corner radius in display pixels. Zero uses square corners.
    pub video_corner_radius: u16,
}

impl Default for PresentationOptions {
    fn default() -> Self {
        Self {
            pane_corner_radius: 9,
            video_corner_radius: 24,
        }
    }
}

/// Application notices can be delivered while the frontend is idle, including SSH.
pub fn run_with_notices(
    launch: Launch,
    custom: fn(&Event) -> Option<Input>,
    notices: fn() -> Option<String>,
) -> Result<()> {
    run_with_options(launch, custom, notices, PresentationOptions::default())
}

pub fn run_with_options(
    launch: Launch,
    custom: fn(&Event) -> Option<Input>,
    notices: fn() -> Option<String>,
    options: PresentationOptions,
) -> Result<()> {
    run_impl(launch, custom, true, None, notices, options, None, None)
}

/// Frontend owned by a local terminal integration, attached through the existing SSH TTY.
pub fn run_terminal_socket_with_events(
    path: &std::path::Path,
    custom: fn(&Event) -> Option<Input>,
) -> Result<()> {
    run_terminal_socket_with_options(path, custom, PresentationOptions::default())
}

pub fn run_terminal_socket_with_options(
    path: &std::path::Path,
    custom: fn(&Event) -> Option<Input>,
    options: PresentationOptions,
) -> Result<()> {
    let launch = Launch {
        executable: String::new(),
        host: None,
        ssh_config: None,
        session: "terminal".into(),
        directory: None,
        attach_only: true,
        play: None,
    };
    run_impl(
        launch,
        custom,
        true,
        Some(path),
        || None,
        options,
        None,
        None,
    )
}

/// Mode and its local-only preference file. Explicit launch overrides do not persist.
#[derive(Clone)]
pub struct PresentationPreference {
    pub cells: bool,
    pub path: std::path::PathBuf,
}
pub fn run_with_preference(
    launch: Launch,
    custom: fn(&Event) -> Option<Input>,
    notices: fn() -> Option<String>,
    options: PresentationOptions,
    preference: PresentationPreference,
) -> Result<()> {
    run_impl(
        launch,
        custom,
        true,
        None,
        notices,
        options,
        Some(preference),
        None,
    )
}
pub fn run_terminal_socket_with_preference(
    path: &std::path::Path,
    custom: fn(&Event) -> Option<Input>,
    options: PresentationOptions,
    preference: PresentationPreference,
) -> Result<()> {
    run_impl(
        Launch {
            executable: String::new(),
            host: None,
            ssh_config: None,
            session: "terminal".into(),
            directory: None,
            attach_only: true,
            play: None,
        },
        custom,
        true,
        Some(path),
        || None,
        options,
        Some(preference),
        None,
    )
}
/// Receive exports only inside a directory explicitly chosen by this frontend.
pub fn run_with_exports(launch: Launch, directory: std::path::PathBuf) -> Result<()> {
    run_impl(
        launch,
        |_| None,
        false,
        None,
        || None,
        PresentationOptions::default(),
        None,
        Some(directory),
    )
}
#[allow(clippy::too_many_arguments)]
fn run_impl(
    launch: Launch,
    custom: fn(&Event) -> Option<Input>,
    terminal_extensions: bool,
    terminal_socket: Option<&std::path::Path>,
    notices: fn() -> Option<String>,
    options: PresentationOptions,
    preference: Option<PresentationPreference>,
    export_directory: Option<std::path::PathBuf>,
) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("An interactive terminal is required to attach an application session");
    }
    let mut drop_bridge = super::drop_bridge::Bridge::new(
        (launch.host.is_some() || terminal_socket.is_some()) && terminal_extensions,
    );
    let mut font = super::font::probe().configured();
    let terminal_background = super::terminal_theme::background();
    let graphics = crate::graphics::Graphics::probe_if_tty(crate::graphics::Mode::Auto);
    let mut capabilities = super::capabilities::Capabilities::detected(&graphics);
    capabilities.file_exports = export_directory.is_some();
    let mut exports = export_directory.map(super::exports::Receiver::new);
    capabilities.terminal_background = terminal_background.map(|c| [c.r, c.g, c.b]);
    capabilities.local_media = cfg!(feature = "media")
        && launch.host.is_none()
        && terminal_socket.is_none()
        && std::env::var_os("SSH_CONNECTION").is_none();
    capabilities.video = cfg!(feature = "media");
    capabilities.presentation_switch = preference.is_some();
    let environment = crate::presentation::Environment::detect();
    let profile = capabilities.profile(
        &environment,
        graphics.name(),
        terminal_socket.is_some() || launch.host.is_some(),
    );
    let graphical_available = profile.native;
    capabilities.local_frames =
        graphical_available && launch.host.is_none() && terminal_socket.is_none();
    capabilities.streamed_frames = graphical_available;
    capabilities.canvas_video &= graphical_available;
    capabilities.canvas_video_high_quality &= capabilities.canvas_video;
    capabilities.key_event_types = graphical_available
        && crate::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if profile.images == crate::presentation::Images::None {
        capabilities.video = false;
        capabilities.video_player = false;
        capabilities.original_media = false;
    }
    let mut pixels = graphical_available && !preference.as_ref().is_some_and(|p| p.cells);
    capabilities.cell_presentation = Some(!pixels);
    // SGR 1016 uses the same event encoding as 1006, with pixel coordinates.
    // Enable it only on a known Kitty terminal with measured pixel geometry;
    // graphics support alone is not evidence of pixel mouse support.
    capabilities.pixel_mouse = super::mouse::supported(
        graphical_available,
        capabilities.pixel_geometry == super::capabilities::PixelGeometry::Measured,
        std::env::var("TERM").ok().as_deref(),
        std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some(),
    );
    tracing::info!(?capabilities, "Terminal presentation capabilities");
    let cell = graphics.cell_size().unwrap_or((10, 20));
    font.cell = Some(cell);
    let mut size = viewport(1, cell)?;
    let mut terminal_grid = (size.columns, size.rows);
    let pixel_scale = if graphical_available {
        std::env::var("STAR_GRAPHICS_SCALE")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|n| (100..=200).contains(n))
            .unwrap_or(100)
    } else {
        100
    };
    size = scaled_viewport(size, if pixels { pixel_scale } else { 100 });
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
    let mut renderer = Frontend::spawn(pixels, font.clone(), options)?;
    let mut switch_supported = false;
    let mut persist_generation = None;
    let mut media = super::media::Frontend::new(capabilities.local_media);
    let mut cell_video = super::cell_video::CellVideo::new(graphics, profile.clone());
    let mut video_layer =
        super::video_presenter::VideoPresenter::with_corner_radius(options.video_corner_radius)?;
    let window = super::window::Window::new()?;
    let mut direct_play = launch.play.clone();
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
    let _pixel_mouse = super::mouse::Mode::enable(capabilities.pixel_mouse)?;
    use std::io::Write;
    let mut presenter = PresenterGuard(KittyPresenter::default());
    let mut pointer = super::pointer::Pointer::default();
    let mut hover_point = None;
    let mut hover_pixel = None;
    let mut hovered_row = None;
    let mut focused = true;
    let mut mouse_down = false;
    let mut pending_event = None;
    let mut resize_handles = Vec::new();
    let mut pointer_regions = Vec::new();
    let mut pointer_placements: Vec<super::placement::Placement> = Vec::new();
    let mut assets = super::assets::FrontendAssets::default();
    let mut epoch: Option<String> = None;
    // Keep startup outside the pixel renderer. Its first capture must belong
    // to the application, rather than a competing placeholder scene.
    io::stdout().write_all(b"\x1b[HConnecting to application session...")?;
    io::stdout().flush()?;
    let mut last_scene: Option<Scene> = None;
    let mut shown: Option<(u64, u64)> = None;
    let mut shown_scene: Option<std::sync::Arc<Scene>> = None;
    let mut id = 0u64;
    let mut connected = false;
    let mut last_reply = Instant::now();
    let mut ping = Instant::now();
    let mut last_input = Instant::now();
    let mut geometry_probe = Instant::now();
    let mut reconnect = Instant::now() + Duration::from_secs(5);
    let mut fatal = None;
    let mut canvases =
        super::canvas_frontend::Frontend::new()?.with_terminal_files(!environment.ssh);
    let mut embedded = super::embedded_input::Router::default();
    let mut capture_keyboard = false;
    let mut frames = 0u64;
    let mut bytes = 0u64;
    loop {
        if connected {
            if let (Some(c), Some((revision, generation))) = (&connection, shown) {
                if let Some(message) = notices() {
                    id += 1;
                    c.send(ClientMessage::Input {
                        id,
                        revision,
                        generation,
                        input: Input::Notice { message },
                    })?;
                }
            }
        }
        let controls = connection
            .as_ref()
            .map(|c| c.messages.try_iter().take(64).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut closed = false;
        for message in controls {
            last_reply = Instant::now();
            match message {
                ServerMessage::CanvasStream { frame } => {
                    anyhow::ensure!(
                        capabilities.streamed_frames,
                        "Streamed previews were not negotiated"
                    );
                    let first = canvases.receive_encoded(&frame)?;
                    if let Some(c) = &connection {
                        c.send(ClientMessage::CanvasAck {
                            stream: frame.stream,
                            generation: frame.generation,
                            sequence: frame.sequence,
                        })?;
                    }
                    if first {
                        if let Some(scene) = &last_scene {
                            renderer.scene(scene)?;
                        }
                    }
                }
                ServerMessage::CanvasFrame { frame } => {
                    let first = canvases.receive(&frame, capabilities.local_frames)?;
                    if let Some(c) = &connection {
                        c.send(ClientMessage::CanvasAck {
                            stream: frame.stream,
                            generation: frame.generation,
                            sequence: frame.sequence,
                        })?;
                    }
                    // Redraw the transparent live region once when the first frame arrives.
                    if first {
                        if let Some(scene) = &last_scene {
                            renderer.scene(scene)?;
                        }
                    }
                }
                ServerMessage::CanvasClose { stream, generation } => {
                    canvases.close(&stream, generation, &mut io::stdout().lock())?;
                    if let Some(scene) = &last_scene {
                        renderer.scene(scene)?;
                    }
                }
                ServerMessage::TogglePresentation => {
                    if !switch_supported || preference.is_none() {
                        continue;
                    }
                    if !graphical_available {
                        if let Some(c) = &connection {
                            id += 1;
                            let (revision, generation) = shown.unwrap_or((0, size.generation));
                            c.send(ClientMessage::Input {
                                id,
                                revision,
                                generation,
                                input: Input::Notice {
                                    message:
                                        "Graphical presentation requires a Kitty graphics terminal"
                                            .into(),
                                },
                            })?;
                        }
                        continue;
                    }
                    let next = Frontend::spawn(!pixels, font.clone(), options)?;
                    pixels = !pixels;
                    renderer = next;
                    cell_video.reset();
                    capabilities.cell_presentation = Some(!pixels);
                    size = scaled_viewport(
                        viewport(size.generation + 1, cell)?,
                        if pixels { pixel_scale } else { 100 },
                    );
                    presenter.clear(&mut io::stdout().lock())?;
                    video_layer.clear(&mut io::stdout().lock())?;
                    io::stdout().write_all(b"\x1b[2J\x1b[H")?;
                    shown = None;
                    shown_scene = None;
                    last_scene = None;
                    pointer.reset(&mut io::stdout().lock())?;
                    pointer_placements.clear();
                    pointer_regions.clear();
                    resize_handles.clear();
                    if let Some(c) = &connection {
                        c.send(ClientMessage::Presentation {
                            viewport: size,
                            cells: !pixels,
                        })?;
                    }
                    persist_generation = Some(size.generation);
                }
                ServerMessage::Fullscreen { enabled } => window.set(enabled),
                ServerMessage::Export { message } => {
                    if let (Some(receiver), Some(c)) = (&mut exports, &connection) {
                        c.send(ClientMessage::Export {
                            message: receiver.receive(message),
                        })?;
                    }
                }
                ServerMessage::Media { message } => {
                    if matches!(
                        &message,
                        super::media::ToClient::CanvasVideoOpen { .. }
                            | super::media::ToClient::CanvasVideoPacket { .. }
                            | super::media::ToClient::CanvasVideoClose { .. }
                    ) {
                        anyhow::ensure!(
                            capabilities.canvas_video,
                            "Live video decoding was not negotiated"
                        );
                    }
                    if let super::media::ToClient::CanvasVideoClose {
                        stream, generation, ..
                    } = &message
                    {
                        canvases.close(stream, *generation, &mut io::stdout().lock())?;
                    }
                    if let Some(c) = &connection {
                        media.receive(message, &c.input)?;
                    }
                }
                ServerMessage::Hello {
                    version,
                    epoch: e,
                    video_player,
                    presentation_switch,
                    ..
                } => {
                    switch_supported = presentation_switch;
                    if direct_play.is_some() && !video_player {
                        fatal = Some(
                            "Update the host graphical package for direct movie launch".into(),
                        );
                        break;
                    }
                    if version != VERSION {
                        fatal = Some("Remote protocol version does not match".to_string());
                        break;
                    }
                    if epoch.as_ref().is_some_and(|old| old != &e) {
                        if let Some(receiver) = &mut exports {
                            receiver.reset();
                        }
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
                    shown_scene = None;
                }
                ServerMessage::LaunchMovie { path } => direct_play = Some(path),
                ServerMessage::Clipboard { text } => renderer.clipboard(&text)?,
                ServerMessage::Asset { id, png } => {
                    assets.insert(id, png)?;
                    // Control and latest-scene queues are independent. Hydrate
                    // both late scenes and late assets from the same cache.
                    if let Some(scene) = &mut last_scene {
                        if assets.hydrate(scene) {
                            renderer.scene(&super::pointer::paint_hover(scene, hovered_row))?;
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
                assets.hydrate(&mut scene);
                if video_layer.frame.as_ref().is_some_and(|(id, _)| {
                    !scene
                        .components
                        .iter()
                        .any(|c| matches!(c,Component::Image{id:key,..}if key==id))
                }) {
                    video_layer.frame = None;
                    video_layer.clear(&mut io::stdout().lock())?;
                }
                hovered_row = if pixels {
                    super::pointer::hovered(&scene, hover_point, hover_pixel)
                } else {
                    None
                };
                renderer.scene(&super::pointer::paint_hover(&scene, hovered_row))?;
                last_scene = Some(scene);
            }
        }
        if let (Some(c), Some(scene)) = (&connection, &last_scene) {
            for message in window.notices.try_iter() {
                id += 1;
                c.send(ClientMessage::Input {
                    id,
                    revision: scene.revision,
                    generation: size.generation,
                    input: Input::Notice { message },
                })?;
            }
            if let Some(path) = direct_play.take() {
                id += 1;
                c.send(ClientMessage::Input {
                    id,
                    revision: scene.revision,
                    generation: size.generation,
                    input: Input::Play { path },
                })?;
            }
        }
        if let Some(c) = &connection {
            if canvases.video_ready() {
                if let Some((stream, generation, decoded)) = media.canvas_tick(&c.input) {
                    if canvases.receive_pixels(stream, generation, decoded)? {
                        if let Some(scene) = &last_scene {
                            renderer.scene(scene)?;
                        }
                    }
                }
            }
            if let Some((id, decoded)) = media
                .tick(&c.input)
                .filter(|_| profile.images != crate::presentation::Images::None)
            {
                video_layer.frame = Some((id, decoded));
                if let Some(scene) = &shown_scene {
                    present_video(
                        &mut video_layer,
                        &mut cell_video,
                        pixels,
                        capabilities.image_transport,
                        false,
                        scene,
                        Viewport {
                            columns: terminal_grid.0,
                            rows: terminal_grid.1,
                            ..size
                        },
                        &mut io::stdout().lock(),
                    )?;
                }
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
                    scene: frame_scene,
                } if generation == size.generation && width > 0 && height > 0 => {
                    let presentation_started = Instant::now();
                    if pixels {
                        if shown.is_none() {
                            let mut out = io::stdout().lock();
                            video_layer.clear(&mut out)?;
                            canvases.invalidate();
                            out.write_all(b"\x1b[2J\x1b[H")?;
                        }
                        bytes += presenter.present_pixels(
                            {
                                let frame = frame_pixels.context("Native frame has no pixels")?;
                                video_layer.mask(
                                    &frame_scene,
                                    canvases.mask(&frame_scene, frame),
                                    Viewport {
                                        columns: terminal_grid.0,
                                        rows: terminal_grid.1,
                                        ..size
                                    },
                                )
                            },
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
                    {
                        let scene = &frame_scene;
                        if !pixels {
                            cell_video.assets(
                                scene,
                                video_layer.frame.as_ref().map(|(id, _)| id.as_str()),
                                &mut io::stdout().lock(),
                            )?;
                        }
                        present_video(
                            &mut video_layer,
                            &mut cell_video,
                            pixels,
                            capabilities.image_transport,
                            true,
                            scene,
                            Viewport {
                                columns: terminal_grid.0,
                                rows: terminal_grid.1,
                                ..size
                            },
                            &mut io::stdout().lock(),
                        )?;
                    }
                    if persist_generation == Some(generation) {
                        if let Some(pref) = &preference {
                            if let Err(error) = crate::config::edit::set(
                                &pref.path,
                                "ui",
                                "presentation",
                                &crate::config::edit::Value::Str(
                                    if pixels { "graphical" } else { "cells" }.into(),
                                ),
                            ) {
                                tracing::warn!(%error,"Could not save presentation preference");
                                if let Some(c) = &connection {
                                    id += 1;
                                    c.send(ClientMessage::Input {
                                        id,
                                        revision,
                                        generation,
                                        input: Input::Notice {
                                            message: format!(
                                                "Could not save presentation preference: {error}"
                                            ),
                                        },
                                    })?;
                                }
                            }
                        }
                        persist_generation = None;
                    }
                    frames += 1;
                    shown = Some((revision, generation));
                    if connected {
                        let scene = &frame_scene;
                        resize_handles.clone_from(&scene.resize_handles);
                        pointer_regions = super::pointer::clickable_regions(scene);
                        pointer_placements.clone_from(&scene.placements);
                    }
                    shown_scene = Some(frame_scene);
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
            // The worker can fail after this iteration drained its output.
            // It queues the cause before exiting; do not replace it with a
            // generic lifecycle error just because is_finished won the race.
            fatal.get_or_insert_with(|| stopped_renderer_message(renderer.output()));
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
            embedded.release();
            canvases.clear(&mut io::stdout().lock())?;
            if capture_keyboard {
                io::stdout().write_all(b"\x1b[=5u")?;
                capture_keyboard = false;
            }
            media.reset();
            if let Some(receiver) = &mut exports {
                receiver.reset();
            }
            window.set(false);
            video_layer.frame = None;
            video_layer.clear(&mut io::stdout().lock())?;
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
            shown_scene = None;
            reconnect = Instant::now() + Duration::from_secs(5);
            if let Some(mut scene) = last_scene.clone() {
                scene.resize_handles.clear();
                scene.pointer_regions.clear();
                scene.spans.push(Span{x:1,y:size.rows.saturating_sub(1),text:"Disconnected · remote operations continue · Ctrl+R reconnect · Ctrl+Q detach".into(),foreground:"#f38ba8".into(),background:scene.background.clone(),bold:true,modifiers:0});
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
        let cinema = last_scene.as_ref().is_some_and(|scene| {
            scene.components.iter().any(|c| {
                matches!(c, Component::Image { rect, id, .. } if id.starts_with("video-")
                && rect.x == 0 && rect.y == 0
                && rect.width == scene.viewport.columns && rect.height == scene.viewport.rows)
            })
        });
        window.cursor(
            cinema
                && connected
                && focused
                && !mouse_down
                && media.video_playing()
                && last_input.elapsed() >= Duration::from_secs(3),
        );
        if let Some(scene) = &shown_scene {
            if pixels {
                if shown.is_some() {
                    bytes += presenter.maintain(&mut io::stdout().lock())? as u64;
                }
                canvases.present(
                    scene,
                    Viewport {
                        columns: terminal_grid.0,
                        rows: terminal_grid.1,
                        ..size
                    },
                    &mut io::stdout().lock(),
                )?;
                // Paused video also owns disposable terminal placements.
                present_video(
                    &mut video_layer,
                    &mut cell_video,
                    pixels,
                    capabilities.image_transport,
                    false,
                    scene,
                    Viewport {
                        columns: terminal_grid.0,
                        rows: terminal_grid.1,
                        ..size
                    },
                    &mut io::stdout().lock(),
                )?;
            }
        }
        let poll_ms = if canvases.active()
            || media.video_playing()
            || rendering
            || last_input.elapsed() < Duration::from_millis(100)
        {
            2
        } else {
            16
        };
        // SIGWINCH notifications can be coalesced during a drag. Recheck the
        // settled geometry even when the final resize notification was lost.
        if pending_event.is_none() && geometry_probe.elapsed() >= Duration::from_millis(250) {
            geometry_probe = Instant::now();
            if let Ok(measured) = viewport(size.generation, cell) {
                if (measured.columns, measured.rows) != terminal_grid
                    || measured.width != size.width
                    || measured.height != size.height
                {
                    pending_event = Some(Event::Resize(measured.columns, measured.rows));
                }
            }
        }
        if pending_event.is_some() || event::poll(Duration::from_millis(poll_ms))? {
            last_input = Instant::now();
            window.cursor(false);
            let first = match pending_event.take() {
                Some(event) => event,
                None => event::read()?,
            };
            let (event, boundary) = coalesce_motion(first, || {
                if event::poll(Duration::ZERO)? {
                    event::read().map(Some)
                } else {
                    Ok(None)
                }
            })?;
            pending_event = boundary;
            let mut input = custom(&event);
            let routed = last_scene
                .as_ref()
                .filter(|_| pixels && (capabilities.local_frames || capabilities.streamed_frames))
                .map(|scene| embedded.route(&event, scene, capabilities.pixel_mouse));
            if let Some(release) = routed.as_ref().and_then(|r| r.release.clone()) {
                if let Some(c) = &connection {
                    let (revision, generation) = shown.unwrap_or((0, size.generation));
                    id += 1;
                    c.send(ClientMessage::Input {
                        id,
                        revision,
                        generation,
                        input: release,
                    })?;
                }
            }
            if let Some(change) = routed.as_ref().and_then(|r| r.keyboard) {
                if capabilities.key_event_types && change != capture_keyboard {
                    io::stdout().write_all(if change { b"\x1b[=15u" } else { b"\x1b[=5u" })?;
                    io::stdout().flush()?;
                    capture_keyboard = change;
                }
            }
            if routed.as_ref().is_some_and(|r| r.consumed) {
                input = routed.and_then(|r| r.input);
            } else {
                match event {
                    Event::Key(k) if k.kind == crate::crossterm::event::KeyEventKind::Press => {
                        pointer.reset(&mut io::stdout().lock())?;
                        if k.modifiers.contains(KeyModifiers::CONTROL)
                            && k.code == KeyCode::Char('l')
                        {
                            if pixels {
                                bytes += presenter.repaint(&mut io::stdout().lock())? as u64;
                            } else {
                                renderer = Frontend::spawn(false, font.clone(), options)?;
                                io::stdout().write_all(b"\x1b[2J\x1b[H")?;
                            }
                            canvases.invalidate();
                            video_layer.invalidate();
                            cell_video.reset();
                            if let Some(scene) = &last_scene {
                                renderer.scene(&super::pointer::paint_hover(scene, hovered_row))?;
                            }
                            continue;
                        }
                        if !connected
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && k.code == KeyCode::Char('q')
                        {
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
                        match m.kind {
                            MouseEventKind::Down(_) => mouse_down = true,
                            MouseEventKind::Up(_) => mouse_down = false,
                            _ => {}
                        }
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
                        let pixel = if capabilities.pixel_mouse {
                            [u32::from(m.column), u32::from(m.row)]
                        } else {
                            [
                                terminal_pixel(m.column, terminal_grid.0, size.width),
                                terminal_pixel(m.row, terminal_grid.1, size.height),
                            ]
                        };
                        let logical_point = if capabilities.pixel_mouse {
                            super::mouse::logical(pixel, size)
                        } else {
                            [
                                logical_coordinate(m.column, terminal_grid.0, size.columns),
                                logical_coordinate(m.row, terminal_grid.1, size.rows),
                            ]
                        };
                        let logical = if pointer_placements.is_empty() {
                            Some((logical_point[0], logical_point[1]))
                        } else {
                            pointer_placements
                                .iter()
                                .rev()
                                .find_map(|p| p.pointer_pixels(pixel[0], pixel[1], false))
                        };
                        hover_point = if mouse_down { None } else { logical };
                        hover_pixel = (!mouse_down).then_some(pixel);
                        let previous_hover = hovered_row;
                        if pixels {
                            if let Some(scene) = &last_scene {
                                let next = super::pointer::hovered(scene, hover_point, hover_pixel);
                                if next != hovered_row {
                                    hovered_row = next;
                                    renderer
                                        .scene(&super::pointer::paint_hover(scene, hovered_row))?;
                                }
                            }
                        }
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
                            x: logical_point[0],
                            y: logical_point[1],
                            modifiers: m.modifiers.bits(),
                        });
                        if action == "move"
                            && matches!((previous_hover, hovered_row),
                                (Some(super::pointer::HoverTarget::Surface { component: a, .. }),
                                 Some(super::pointer::HoverTarget::Surface { component: b, .. })) if a == b)
                        {
                            input = None;
                        }
                    }
                    Event::Paste(text) => input = Some(Input::Paste { text }),
                    Event::FocusLost => {
                        focused = false;
                        mouse_down = false;
                        hover_point = None;
                        hover_pixel = None;
                        hovered_row = None;
                        if pixels {
                            if let Some(scene) = &last_scene {
                                renderer.scene(&super::pointer::paint_hover(scene, hovered_row))?;
                            }
                        }
                        pointer.reset(&mut io::stdout().lock())?;
                        input = Some(Input::CancelPointer);
                    }
                    Event::Resize(..) => {
                        hover_point = None;
                        hover_pixel = None;
                        hovered_row = None;
                        pointer.reset(&mut io::stdout().lock())?;
                        resize_handles.clear();
                        pointer_regions.clear();
                        pointer_placements.clear();
                        geometry_probe = Instant::now();
                        // The terminal may have clipped or moved an existing Kitty
                        // placement. Its cached pixels no longer prove it is visible.
                        presenter.clear(&mut io::stdout().lock())?;
                        video_layer.clear(&mut io::stdout().lock())?;
                        canvases.hide(&mut io::stdout().lock())?;
                        cell_video.reset();
                        size = viewport(size.generation + 1, cell)?;
                        terminal_grid = (size.columns, size.rows);
                        size = scaled_viewport(size, if pixels { pixel_scale } else { 100 });
                        // Geometry and embedded hit targets belong to the old
                        // generation until the controller supplies a new scene.
                        last_scene = None;
                        shown = None;
                        shown_scene = None;
                        input = Some(Input::Resize { viewport: size });
                    }
                    Event::FocusGained => {
                        focused = true;
                        if pixels {
                            bytes += presenter.repaint(&mut io::stdout().lock())? as u64;
                        }
                        canvases.invalidate();
                        video_layer.invalidate();
                    }
                    _ => {}
                }
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
    if capture_keyboard {
        io::stdout().write_all(b"\x1b[=5u")?;
    }
    canvases.clear(&mut io::stdout().lock())?;
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

// A terminal can queue hundreds of hover positions while a frame is being
// presented. Only the latest contiguous position matters. Preserve every
// button, wheel, key and geometry event. Adjacent drags may coalesce only
// while their button and modifier state are unchanged.
fn coalesce_motion(
    mut event: Event,
    mut next: impl FnMut() -> io::Result<Option<Event>>,
) -> io::Result<(Event, Option<Event>)> {
    let motion = match &event {
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) => {
            Some((m.kind, m.modifiers))
        }
        _ => None,
    };
    if let Some((kind, modifiers)) = motion {
        // Bound the drain so continuous motion cannot starve frame delivery.
        for _ in 0..255 {
            match next()? {
                Some(Event::Mouse(m)) if m.kind == kind && m.modifiers == modifiers => {
                    event = Event::Mouse(m)
                }
                Some(boundary) => return Ok((event, Some(boundary))),
                None => break,
            }
        }
    }
    Ok((event, None))
}

#[cfg(test)]
mod tests {
    #[test]
    fn stalled_pipe_coalesces_drag_and_delivers_every_release_and_control() {
        use std::sync::{Arc, Mutex};
        struct GatedWriter {
            started: Option<crossbeam_channel::Sender<()>>,
            gate: crossbeam_channel::Receiver<()>,
            bytes: Arc<Mutex<Vec<u8>>>,
        }
        impl std::io::Write for GatedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Some(started) = self.started.take() {
                    started.send(()).unwrap();
                    self.gate.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                self.bytes.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (started, ready) = bounded(1);
        let (release, gate) = bounded(1);
        let bytes = Arc::new(Mutex::new(vec![]));
        let connection = Connection::streams(
            GatedWriter {
                started: Some(started),
                gate,
                bytes: bytes.clone(),
            },
            std::io::empty(),
            None,
            None,
        )
        .unwrap();
        let pointer = |id, action: &str| ClientMessage::Input {
            id,
            revision: 1,
            generation: 1,
            input: Input::Pointer {
                action: action.into(),
                button: 0,
                modifiers: 0,
                pixel: None,
                x: 1,
                y: 1,
            },
        };
        connection.send(pointer(1, "down")).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for id in 2..10002 {
            connection.send(pointer(id, "drag")).unwrap();
        }
        connection.send(pointer(10002, "up")).unwrap();
        for _ in 0..40 {
            connection.send(ClientMessage::Ping).unwrap();
        }
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let count = bytes
                .lock()
                .unwrap()
                .iter()
                .filter(|b| **b == b'\n')
                .count();
            if count == 43 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Controls stranded after writer wakeup"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut data = std::io::Cursor::new(bytes.lock().unwrap().clone());
        let mut messages = vec![];
        while let Some(message) = read_message::<ClientMessage>(&mut data).unwrap() {
            messages.push(message);
        }
        assert!(matches!(
            &messages[1],
            ClientMessage::Input { id: 10001, .. }
        ));
        assert!(
            matches!(&messages[2],ClientMessage::Input{id:10002,input:Input::Pointer{action,..},..}if action=="up")
        );
    }
    /// Run only against an isolated ALSA null device. Deliberately never poll
    /// Connection.messages or frames: terminal processing is suspended.
    #[test]
    #[ignore]
    #[cfg(feature = "audio-output")]
    fn pcm_cache_refills_without_polling_the_ui() {
        use super::super::media::{ToClient, ToHost};
        use super::*;
        use base64::Engine as _;
        assert!(std::env::var_os("STAR_PCM_DEVICE_TEST").is_some());
        let (mut host, client) = std::os::unix::net::UnixStream::pair().unwrap();
        host.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        let connection =
            Connection::streams(client.try_clone().unwrap(), client, None, None).unwrap();
        let mut input = BufReader::new(host.try_clone().unwrap());
        let mut send =
            |message| write_message(&ServerMessage::Media { message }, &mut host).unwrap();
        send(ToClient::PcmOpen {
            session: 11,
            epoch: 3,
            spec: crate::pcm::Spec {
                sample_rate: 48000,
                channels: 2,
                channel_mask: 3,
                format: crate::pcm::SampleFormat::I32,
                valid_bits: Some(32),
            },
            compatibility: false,
        });
        send(ToClient::PcmCache {
            session: 11,
            epoch: 3,
        });
        send(ToClient::PcmPause {
            session: 11,
            epoch: 3,
            paused: true,
        });
        let started = Instant::now();
        let data = base64::engine::general_purpose::STANDARD.encode(vec![0; 960 * 8]);
        let mut sent = 0;
        let mut end = false;
        loop {
            let Some(ClientMessage::Media { message }) =
                read_message::<ClientMessage>(&mut input).unwrap()
            else {
                continue;
            };
            match message {
                ToHost::AudioCredit { blocks, .. } => {
                    for _ in 0..blocks {
                        if sent < 300 {
                            send(ToClient::PcmChunk {
                                session: 11,
                                epoch: 3,
                                format: crate::pcm::SampleFormat::I32,
                                processed: false,
                                data: data.clone(),
                            });
                            sent += 1;
                        } else if !end {
                            send(ToClient::PcmEnd {
                                session: 11,
                                epoch: 3,
                            });
                            end = true;
                        }
                    }
                }
                ToHost::PcmBuffered { frames, .. } if frames == 300 * 960 => break,
                ToHost::PcmProgress { frames, .. } => assert_eq!(frames, 0, "paused cache played"),
                ToHost::PcmFinished { .. } => panic!("paused cache finished"),
                ToHost::AudioError { message, .. } => panic!("{message}"),
                _ => {}
            }
            assert!(started.elapsed() < Duration::from_secs(3));
        }
        assert_eq!(sent, 300);
        if !end {
            send(ToClient::PcmEnd {
                session: 11,
                epoch: 3,
            });
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        send(ToClient::PcmPause {
            session: 11,
            epoch: 3,
            paused: false,
        });
        loop {
            if let Some(ClientMessage::Media { message }) =
                read_message::<ClientMessage>(&mut input).unwrap()
            {
                match message {
                    ToHost::PcmFinished { .. } => break,
                    ToHost::AudioError { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
            assert!(started.elapsed() < Duration::from_secs(10));
        }
        drop(connection);
    }
    #[test]
    fn motion_bursts_keep_the_latest_position_without_reordering_actions() {
        use crate::crossterm::event::{
            Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };
        use std::collections::VecDeque;
        let mouse = |kind, column| {
            Event::Mouse(MouseEvent {
                kind,
                column,
                row: 5,
                modifiers: KeyModifiers::NONE,
            })
        };
        let moved = |column| mouse(MouseEventKind::Moved, column);
        let boundaries = [
            mouse(MouseEventKind::Down(MouseButton::Left), 10),
            mouse(MouseEventKind::Drag(MouseButton::Left), 11),
            mouse(MouseEventKind::Drag(MouseButton::Left), 12),
            mouse(MouseEventKind::Up(MouseButton::Left), 13),
            mouse(MouseEventKind::ScrollDown, 14),
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            Event::Resize(100, 40),
            Event::FocusLost,
            Event::Paste("text".into()),
        ];
        let mut events = VecDeque::new();
        let mut expected = Vec::new();
        for boundary in boundaries {
            events.extend([moved(1), moved(2), moved(3), boundary.clone()]);
            expected.extend([moved(3), boundary]);
        }
        let mut actual = Vec::new();
        let mut pending = None;
        while let Some(first) = pending.take().or_else(|| events.pop_front()) {
            let (event, boundary) =
                super::coalesce_motion(first, || Ok(events.pop_front())).unwrap();
            actual.push(event);
            pending = boundary;
        }
        assert_eq!(actual, expected);
        let mut events: VecDeque<_> = (1..1000).map(moved).collect();
        let (latest, boundary) =
            super::coalesce_motion(moved(0), || Ok(events.pop_front())).unwrap();
        assert_eq!(latest, moved(255));
        assert!(boundary.is_none());
        assert_eq!(
            events.front(),
            Some(&moved(256)),
            "continuous motion must yield to rendering"
        );
    }

    #[test]
    fn drag_bursts_coalesce_without_crossing_buttons_modifiers_or_release() {
        use crate::crossterm::event::MouseEvent;
        let drag = |button, modifiers, x| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(button),
                modifiers,
                column: x,
                row: 1,
            })
        };
        let first = drag(MouseButton::Left, KeyModifiers::NONE, 1);
        let boundary = drag(MouseButton::Left, KeyModifiers::SHIFT, 100);
        let mut pending = std::collections::VecDeque::from([
            drag(MouseButton::Left, KeyModifiers::NONE, 2),
            drag(MouseButton::Left, KeyModifiers::NONE, 3),
            boundary.clone(),
        ]);
        let (latest, held) = coalesce_motion(first, || Ok(pending.pop_front())).unwrap();
        assert_eq!(latest, drag(MouseButton::Left, KeyModifiers::NONE, 3));
        assert_eq!(held, Some(boundary));
    }

    #[test]
    fn finished_worker_preserves_its_queued_failure_reason() {
        let (tx, rx) = crossbeam_channel::bounded(2);
        tx.send(super::RenderMessage::Ready).unwrap();
        tx.send(super::RenderMessage::Error {
            message: "Native renderer: Sprite scaling requires slice metadata".into(),
        })
        .unwrap();
        drop(tx);
        assert_eq!(
            super::stopped_renderer_message(&rx),
            "Native renderer: Sprite scaling requires slice metadata"
        );
        assert_eq!(
            super::stopped_renderer_message(&rx),
            "Graphical renderer stopped"
        );
    }
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
            attach_only: false,
            play: None,
        }
        .command(None)
        .is_err());
    }
}
