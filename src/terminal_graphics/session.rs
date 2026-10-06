//! Persistent user-owned application sessions; no TCP listener or browser needed.
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};

use super::protocol::*;

/// Implemented by application UI controllers, never by the renderer.
pub trait Controller {
    fn tick(&mut self);
    /// Quiet applications can avoid rebuilding unchanged scenes. Input and
    /// attachment always request a frame independently of this interval.
    fn frame_interval(&self) -> Duration {
        Duration::from_millis(33)
    }
    fn attached(&mut self) {}
    fn capabilities(&mut self, _capabilities: super::capabilities::Capabilities) {}
    fn detached(&mut self) {}
    fn output_pending(&mut self, _pending: bool) {}
    fn scene(&mut self, viewport: Viewport) -> Scene;
    fn input(&mut self, input: Input);
    fn media(&mut self, _message: super::media::ToHost) {}
    fn media_chunks(&mut self) -> Vec<ServerMessage> {
        vec![]
    }
    fn effects(&mut self) -> Vec<ServerMessage> {
        vec![]
    }
    fn closed(&self) -> bool;
    fn shutdown(&mut self);
}

pub fn socket_path(root: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 40
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("Session names must contain 1–40 letters, digits, '-' or '_'");
    }
    let path = root.join(format!("{name}.sock"));
    if path.as_os_str().len() > 100 {
        bail!("Graphical session socket path is too long");
    }
    Ok(path)
}
pub fn private_root(root: &Path) -> Result<()> {
    if !root.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
    }
    let meta = fs::symlink_metadata(root)?;
    if !meta.is_dir() || meta.file_type().is_symlink() || meta.permissions().mode() & 0o077 != 0 {
        bail!("Graphical session directory must be a private directory (mode 0700)");
    }
    Ok(())
}
pub fn list(root: &Path) -> Result<Vec<String>> {
    if !root.exists() {
        return Ok(vec![]);
    }
    private_root(root)?;
    let mut sessions = vec![];
    for item in fs::read_dir(root)? {
        let path = item?.path();
        if path.extension().is_some_and(|x| x == "sock") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                sessions.push(name.to_string());
            }
        }
    }
    sessions.sort();
    Ok(sessions)
}

#[derive(Default)]
struct PresentationBudget {
    enabled: bool,
    outstanding: Option<(u64, u64, Instant, usize)>,
}
impl PresentationBudget {
    fn ready(&self, generation: u64) -> bool {
        !self.enabled
            || self
                .outstanding
                .is_none_or(|(_, old, _, _)| old != generation)
    }
    fn sent(&mut self, revision: u64, generation: u64, bytes: usize) {
        if self.enabled {
            self.outstanding = Some((revision, generation, Instant::now(), bytes));
        }
    }
    fn presented(&mut self, revision: u64, generation: u64) {
        if let Some((sent, geometry, started, bytes)) = self.outstanding {
            if sent == revision && geometry == generation {
                tracing::debug!(
                    revision,
                    generation,
                    bytes,
                    delivery_ms = started.elapsed().as_millis(),
                    "Scene delivery and presentation acknowledged"
                );
                self.outstanding = None;
            }
        }
    }
}

struct Peer {
    socket: UnixStream,
    messages: Receiver<Option<ClientMessage>>,
    control: Sender<ServerMessage>,
    media: Sender<ServerMessage>,
    frames: Sender<ServerMessage>,
    old_frame: Receiver<ServerMessage>,
    client: Option<String>,
    assets: std::sync::Mutex<HashSet<String>>,
    presentation: PresentationBudget,
}
impl Peer {
    fn new(socket: UnixStream) -> Result<Self> {
        struct Setup<'a> {
            socket: &'a UnixStream,
            armed: bool,
        }
        impl Drop for Setup<'_> {
            fn drop(&mut self) {
                if self.armed {
                    let _ = self.socket.shutdown(std::net::Shutdown::Both);
                }
            }
        }
        let mut setup = Setup {
            socket: &socket,
            armed: true,
        };
        // Accepted sockets can inherit the listener's nonblocking mode on BSD.
        // Dedicated reader/writer threads require blocking streams everywhere.
        socket.set_nonblocking(false)?;
        let (tx, messages) = bounded(64);
        let mut reader = BufReader::new(socket.try_clone()?);
        let mut writer = socket.try_clone()?;
        writer.set_write_timeout(Some(Duration::from_secs(10)))?;
        std::thread::Builder::new()
            .name("star-session-input".into())
            .spawn(move || loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if tx.send(Some(message)).is_err() {
                            return;
                        }
                    }
                    _ => {
                        let _ = tx.send(None);
                        return;
                    }
                }
            })?;
        let (control, rx) = bounded(64);
        let (frames, frame_rx) = bounded(1);
        let (media, media_rx) = bounded(8);
        let old_frame = frame_rx.clone();
        std::thread::Builder::new()
            .name("star-session-output".into())
            .spawn(move || loop {
                let message = crossbeam_channel::select_biased! {
                    recv(rx)->m=>match m {Ok(m)=>m,Err(_)=>return},
                    recv(frame_rx)->m=>match m {Ok(m)=>m,Err(_)=>return},
                    recv(media_rx)->m=>match m {Ok(m)=>m,Err(_)=>return},
                };
                if write_message(&message, &mut writer).is_err() {
                    let _ = writer.shutdown(std::net::Shutdown::Both);
                    return;
                }
            })?;
        setup.armed = false;
        drop(setup);
        Ok(Self {
            socket,
            messages,
            control,
            media,
            frames,
            old_frame,
            client: None,
            assets: std::sync::Mutex::new(HashSet::new()),
            presentation: PresentationBudget::default(),
        })
    }
    fn control(&self, message: ServerMessage) -> bool {
        self.control.try_send(message).is_ok()
    }
    fn scene(&mut self, scene: &Scene) -> bool {
        if self.client.is_none() || !self.presentation.ready(scene.viewport.generation) {
            return false;
        }
        let mut scene = scene.clone();
        let mut assets = self.assets.lock().expect("asset cache");
        if assets.len() > 8 {
            assets.clear();
        }
        for component in &mut scene.components {
            if let Component::Image { id, png, .. } = component {
                if let Some(png) = png.take() {
                    if !assets.contains(id) {
                        if !self.control(ServerMessage::Asset {
                            id: id.clone(),
                            png,
                        }) {
                            return false;
                        }
                        assets.insert(id.clone());
                    }
                }
            }
        }
        let revision = scene.revision;
        let generation = scene.viewport.generation;
        let message = ServerMessage::Scene { scene };
        let bytes = if self.presentation.enabled {
            match serde_json::to_vec(&message) {
                Ok(encoded) => encoded.len() + 1,
                Err(_) => return false,
            }
        } else {
            0
        };
        let sent = match self.frames.try_send(message) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(message)) => {
                let _ = self.old_frame.try_recv();
                self.frames.try_send(message).is_ok()
            }
            Err(_) => false,
        };
        if sent {
            self.presentation.sent(revision, generation, bytes);
        }
        sent
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn serve(root: &Path, name: &str, mut controller: impl Controller) -> Result<()> {
    private_root(root)?;
    let path = socket_path(root, name)?;
    // A stale socket is removed only when no running server accepts it.
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            bail!("Session '{name}' is already running");
        }
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let _guard = SocketGuard(path);
    listener.set_nonblocking(true)?;
    let epoch = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let mut viewport = Viewport::default();
    let mut scene = controller.scene(viewport);
    scene.revision = 1;
    let mut peer: Option<Peer> = None;
    let mut candidates: Vec<(Peer, Instant)> = vec![];
    let mut clients: HashMap<String, Admission> = HashMap::new();
    let mut attached = true;
    let mut input_dirty = false;
    let mut effects = VecDeque::new();
    let mut in_flight = HashSet::new();
    let mut effect_id = 0u64;
    let mut painted = Instant::now();
    let mut history = VecDeque::from([(
        scene.revision,
        scene.viewport.generation,
        scene.interaction,
        scene.scroll_interaction,
    )]);
    loop {
        let start = Instant::now();
        match listener.accept() {
            Ok((socket, _)) => {
                // Liveness probes and abandoned connections must not steal the
                // active attachment. Promote only a complete protocol handshake.
                if candidates.len() < 4 {
                    match Peer::new(socket) {
                        Ok(candidate) => candidates.push((candidate, Instant::now())),
                        Err(error) => {
                            tracing::debug!(%error, "Discarding connection that closed during session setup")
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        let mut handshake = None;
        let mut index = 0;
        while index < candidates.len() {
            match candidates[index].0.messages.try_recv() {
                Ok(Some(message @ ClientMessage::Hello { .. })) => {
                    let valid = match &message {
                        ClientMessage::Hello {
                            version,
                            viewport,
                            client,
                            ..
                        } => {
                            *version == VERSION
                                && client.len() <= 128
                                && viewport.validate().is_ok()
                        }
                        _ => false,
                    };
                    let (candidate, _) = candidates.swap_remove(index);
                    if valid {
                        if peer.as_ref().is_some_and(|p| p.client.is_some()) {
                            controller.detached();
                        }
                        effects.clear();
                        in_flight.clear();
                        peer = Some(candidate);
                        handshake = Some(Some(message));
                        attached = true;
                    }
                }
                Ok(_) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    candidates.swap_remove(index);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    if candidates[index].1.elapsed() > Duration::from_secs(5) {
                        candidates.swap_remove(index);
                    } else {
                        index += 1;
                    }
                }
            }
        }
        let messages = handshake
            .into_iter()
            .chain(
                peer.as_ref()
                    .map(|p| p.messages.try_iter().take(64).collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
            .collect::<Vec<_>>();
        for message in messages {
            let Some(message) = message else {
                if peer.as_ref().is_some_and(|p| p.client.is_some()) {
                    controller.detached();
                }
                peer = None;
                break;
            };
            let Some(p) = peer.as_mut() else {
                break;
            };
            match message {
                ClientMessage::Media { message } => controller.media(message),
                ClientMessage::Hello {
                    version,
                    viewport: v,
                    client,
                    capabilities,
                } => {
                    if version != VERSION || client.len() > 128 || v.validate().is_err() {
                        p.control(ServerMessage::Error {
                            message: "Incompatible protocol or viewport".into(),
                        });
                        peer = None;
                        break;
                    }
                    if !clients.contains_key(&client) && clients.len() >= 64 {
                        p.control(ServerMessage::Error {
                            message: "Session client limit reached".into(),
                        });
                        peer = None;
                        break;
                    }
                    clients.entry(client.clone()).or_default();
                    p.client = Some(client);
                    viewport = v;
                    controller.attached();
                    if let Some(capabilities) = capabilities {
                        p.presentation.enabled = capabilities.presentation_ack;
                        controller.capabilities(capabilities);
                    }
                    p.control(ServerMessage::Hello {
                        version: VERSION,
                        video_player: true,
                        session: name.into(),
                        epoch: epoch.clone(),
                    });
                    attached = true;
                }
                ClientMessage::Input {
                    id,
                    revision,
                    generation,
                    input,
                } => {
                    let Some(client) = p.client.as_ref() else {
                        peer = None;
                        break;
                    };
                    let admission = clients.get_mut(client).expect("registered client");
                    let stale_release = matches!(&input,Input::Pointer{action,..} if action=="up")
                        && admission.cancel_release(revision, generation, &scene);
                    let accepted = admission.admit(id, revision, generation, &scene, &input);
                    tracing::debug!(id, accepted, "Graphical input admitted");
                    let input = if stale_release {
                        Input::CancelPointer
                    } else {
                        input
                    };
                    if accepted {
                        input_dirty = true;
                        match input {
                            Input::Resize { viewport: v } => {
                                if let Ok(v) = v.validate() {
                                    viewport = v;
                                }
                            }
                            Input::Detach => {
                                controller.detached();
                                peer = None;
                                break;
                            }
                            input => controller.input(input),
                        }
                    }
                    if !p.control(ServerMessage::Ack { id, accepted }) {
                        peer = None;
                        break;
                    }
                }
                ClientMessage::Presented {
                    revision,
                    generation,
                } => {
                    p.presentation.presented(revision, generation);
                    if let Some((_, _, interaction, scroll)) = history
                        .iter()
                        .find(|(r, g, _, _)| *r == revision && *g == generation)
                    {
                        if let Some(a) = p.client.as_ref().and_then(|c| clients.get_mut(c)) {
                            a.target(revision, generation, *interaction);
                            a.scroll_target(revision, generation, *scroll);
                        }
                    }
                }
                ClientMessage::EffectAck { id } => {
                    in_flight.remove(&id);
                }
                ClientMessage::Ping => {
                    p.control(ServerMessage::Pong);
                }
            }
        }
        controller.output_pending(!effects.is_empty() || !in_flight.is_empty());
        controller.tick();
        if let Some(p) = &peer {
            if p.media.len() <= 6 {
                for message in controller.media_chunks() {
                    let _ = p.media.try_send(message);
                }
            }
        }
        if peer.as_ref().is_some_and(|p| p.client.is_some())
            && ((attached
                && peer
                    .as_ref()
                    .is_some_and(|p| p.presentation.ready(viewport.generation)))
                || input_dirty
                || painted.elapsed() >= controller.frame_interval())
        {
            let mut next = controller.scene(viewport);
            input_dirty = false;
            painted = Instant::now();
            if !next.same_content(&scene) {
                next.revision = scene.revision + 1;
                scene = next;
                attached = true;
            }
            if attached && peer.as_ref().is_some_and(|p| p.client.is_some()) {
                if let Some(p) = &mut peer {
                    attached = !p.scene(&scene);
                    if !attached {
                        let presented = (
                            scene.revision,
                            scene.viewport.generation,
                            scene.interaction,
                            scene.scroll_interaction,
                        );
                        if history.back() != Some(&presented) {
                            history.push_back(presented);
                        }
                        while history.len() > 64 {
                            history.pop_front();
                        }
                    }
                }
            }
        }
        if peer.is_none() {
            effects.clear();
            in_flight.clear();
            let _ = controller.effects();
        } else if effects.len() < 64 {
            effects.extend(controller.effects());
        }
        if let Some(p) = &peer {
            while in_flight.len() < 64 {
                let Some(mut effect) = effects.pop_front() else {
                    break;
                };
                let wire = matches!(effect, ServerMessage::Osc72 { .. });
                if let ServerMessage::Osc72 { id, .. } = &mut effect {
                    effect_id += 1;
                    *id = effect_id;
                }
                if !p.control(effect.clone()) {
                    effects.push_front(effect);
                    break;
                }
                if wire {
                    in_flight.insert(effect_id);
                }
            }
        }
        if controller.closed() {
            if let Some(p) = &peer {
                p.control(ServerMessage::Closed);
                std::thread::sleep(Duration::from_millis(50));
            }
            controller.shutdown();
            return Ok(());
        }
        let remaining = Duration::from_millis(33).saturating_sub(start.elapsed());
        // Input and presentation acknowledgements wake the controller immediately.
        // Keep the timer for worker updates and accepting new attachments.
        if let Some(p) = &peer {
            let mut wait = crossbeam_channel::Select::new();
            wait.recv(&p.messages);
            let _ = wait.ready_timeout(remaining);
        } else {
            std::thread::sleep(remaining);
        }
    }
}

/// Stdio relay is disposable: EOF closes only this attachment, not the session.
pub fn relay(socket: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(socket).context("Connect to graphical session")?;
    let mut output = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut stream);
        let _ = stream.shutdown(std::net::Shutdown::Both);
    });
    let mut stdout = io::stdout().lock();
    let mut bytes = [0; 32768];
    loop {
        let n = output.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        stdout.write_all(&bytes[..n])?;
        // A relay must forward a small handshake immediately, even when the
        // controller's scene is idle and no later write would fill a buffer.
        stdout.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_input_is_acknowledged_without_resending_the_scene() {
        struct Quiet(bool);
        impl Controller for Quiet {
            fn tick(&mut self) {}
            fn frame_interval(&self) -> Duration {
                Duration::from_secs(60)
            }
            fn scene(&mut self, viewport: Viewport) -> Scene {
                Scene::from_buffer(
                    &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                        0,
                        0,
                        viewport.columns,
                        viewport.rows,
                    )),
                    viewport,
                    0,
                )
            }
            fn input(&mut self, input: Input) {
                if matches!(input, Input::Key { code, .. } if code == "char:q") {
                    self.0 = true;
                }
            }
            fn closed(&self) -> bool {
                self.0
            }
            fn shutdown(&mut self) {}
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let socket = socket_path(&root, "quiet").unwrap();
        let server = std::thread::spawn(move || serve(&root, "quiet", Quiet(false)).unwrap());
        let start = Instant::now();
        let mut stream = loop {
            if let Ok(stream) = UnixStream::connect(&socket) {
                break stream;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        write_message(
            &ClientMessage::Hello {
                version: VERSION,
                viewport: Viewport::default(),
                client: "quiet-test".into(),
                capabilities: None,
            },
            &mut stream,
        )
        .unwrap();
        assert!(matches!(
            read_message::<ServerMessage>(&mut reader).unwrap(),
            Some(ServerMessage::Hello { .. })
        ));
        assert!(matches!(
            read_message::<ServerMessage>(&mut reader).unwrap(),
            Some(ServerMessage::Scene { .. })
        ));
        let send_key = |stream: &mut UnixStream, id, code: &str| {
            write_message(
                &ClientMessage::Input {
                    id,
                    revision: 1,
                    generation: 1,
                    input: Input::Key {
                        code: code.into(),
                        modifiers: 0,
                    },
                },
                stream,
            )
            .unwrap()
        };
        send_key(&mut stream, 1, "unknown");
        assert!(matches!(
            read_message::<ServerMessage>(&mut reader).unwrap(),
            Some(ServerMessage::Ack {
                id: 1,
                accepted: true
            })
        ));
        stream
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        assert!(
            read_message::<ServerMessage>(&mut reader).is_err(),
            "ignored input must not trigger a PNG render"
        );
        send_key(&mut stream, 2, "char:q");
        server.join().unwrap();
    }
    #[test]
    fn presentation_budget_preserves_legacy_clients_and_rejects_stale_feedback() {
        let mut budget = PresentationBudget::default();
        budget.sent(1, 1, 4096);
        assert!(budget.ready(1));
        budget.enabled = true;
        budget.sent(1, 1, 4096);
        assert!(!budget.ready(1));
        budget.presented(2, 1);
        budget.presented(1, 2);
        assert!(!budget.ready(1));
        // Resizing must not wait for a frame the frontend now discards.
        assert!(budget.ready(2));
        budget.sent(2, 2, 8192);
        budget.presented(1, 1);
        assert!(!budget.ready(2));
        budget.presented(2, 2);
        assert!(budget.ready(2));
    }

    #[test]
    fn slow_presentation_keeps_only_one_scene_in_flight() {
        let (client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(client);
        let mut peer = Peer::new(server).unwrap();
        peer.client = Some("paced".into());
        peer.presentation.enabled = true;
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(0, 0, 80, 24)),
            Viewport {
                columns: 80,
                rows: 24,
                ..Viewport::default()
            },
            1,
        );
        assert!(peer.scene(&scene));
        scene.revision = 2;
        assert!(!peer.scene(&scene));
        let Some(ServerMessage::Scene { scene: first }) = read_message(&mut reader).unwrap() else {
            panic!("expected the first scene");
        };
        assert_eq!(first.revision, 1);
        assert!(!peer.scene(&scene));
        peer.presentation.presented(1, first.viewport.generation);
        scene.revision = 3;
        assert!(peer.scene(&scene));
        let Some(ServerMessage::Scene { scene: next }) = read_message(&mut reader).unwrap() else {
            panic!("expected the latest scene");
        };
        assert_eq!(next.revision, 3);
        assert!(peer.presentation.outstanding.unwrap().3 > 0);
    }

    #[test]
    fn accepted_nonblocking_stream_waits_for_a_complete_handshake() {
        let (mut client, server) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let peer = Peer::new(server).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        write_message(
            &ClientMessage::Hello {
                capabilities: None,
                version: VERSION,
                viewport: Viewport::default(),
                client: "slow-handshake".into(),
            },
            &mut client,
        )
        .unwrap();
        assert!(matches!(
            peer.messages.recv_timeout(Duration::from_secs(1)).unwrap(),
            Some(ClientMessage::Hello { .. })
        ));
    }
    #[test]
    fn socket_names_cannot_escape_private_directory() {
        assert!(socket_path(Path::new("/tmp/kit"), "../escape").is_err());
        assert!(socket_path(Path::new("/tmp/kit"), "work_2").is_ok());
    }
}
