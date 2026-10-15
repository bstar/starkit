//! Local application/session lifecycle with opaque domain arguments and a persistent controller.
use super::{
    client::{self, Launch},
    protocol::{
        read_message, write_message, ClientMessage, Input, ServerMessage, Viewport, VERSION,
    },
    session::{self, Controller},
};
use anyhow::{ensure, Context, Result};
use std::{
    io::{BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum Arguments {
    Context {
        args: Vec<String>,
        directory: PathBuf,
    },
    // Controllers started before launch directories were recorded remain attachable.
    Legacy(Vec<String>),
}

impl Arguments {
    fn read(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(1_048_577)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 1_048_576,
            "Application arguments exceed limits"
        );
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn domain(&self) -> (&[String], Option<&Path>) {
        match self {
            Self::Context { args, directory } => (args, Some(directory)),
            Self::Legacy(args) => (args, None),
        }
    }
}
pub struct Application {
    name: String,
    root: PathBuf,
}
impl Application {
    pub fn new(name: &str) -> Result<Self> {
        ensure!(
            !name.is_empty()
                && name.len() <= 24
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid application name"
        );
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join(format!("starkit-{}", rustix::process::geteuid().as_raw()))
            });
        Ok(Self {
            name: name.into(),
            root: runtime.join(format!("starkit-{name}")),
        })
    }
    pub fn with_root(mut self, root: PathBuf) -> Self {
        self.root = root;
        self
    }
    /// Close a matching controller using the application's normal quit inputs.
    /// This replaces its attached frontend and waits for orderly cleanup before
    /// returning. A timeout leaves the controller running; it never kills workers
    /// or starts a replacement over resources that are still owned.
    pub fn close_matching(
        &self,
        matches: impl Fn(&[String], Option<&Path>) -> bool,
        inputs: &[Input],
        timeout: Duration,
    ) -> Result<bool> {
        ensure!(
            !inputs.is_empty() && inputs.len() <= 64,
            "Invalid quit inputs"
        );
        ensure!(!timeout.is_zero(), "Session close timeout must be positive");
        let Some(name) = self.matching_session(matches)? else {
            return Ok(false);
        };
        let socket = session::socket_path(&self.root, &name)?;
        let context = self.root.join(format!("{name}.args.json"));
        let start = Instant::now();
        let mut stream = UnixStream::connect(&socket)?;
        stream.set_write_timeout(Some(timeout))?;
        write_message(
            &ClientMessage::Hello {
                version: VERSION,
                viewport: Viewport::default(),
                client: format!(
                    "close-{}-{}",
                    std::process::id(),
                    SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
                ),
                capabilities: None,
            },
            &mut stream,
        )?;
        for (index, input) in inputs.iter().enumerate() {
            write_message(
                &ClientMessage::Input {
                    id: index as u64 + 1,
                    revision: 0,
                    generation: 1,
                    input: input.clone(),
                },
                &mut stream,
            )?;
        }
        let mut reader = BufReader::new(stream);
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            ensure!(
                !remaining.is_zero(),
                "Session '{name}' has not finished closing; reconnect to check its progress"
            );
            reader.get_ref().set_read_timeout(Some(remaining))?;
            match read_message::<ServerMessage>(&mut reader).context(
                "Waiting for the application to finish closing; no replacement was started",
            )? {
                Some(ServerMessage::Closed) | None => break,
                Some(ServerMessage::Error { message }) => anyhow::bail!("{message}"),
                _ => {}
            }
        }
        // Closed precedes resource teardown. Wait for both lifecycle guards,
        // including the application's project lock, before allowing a restart.
        while socket.exists() || context.exists() {
            ensure!(
                start.elapsed() < timeout,
                "Session '{name}' cleanup has not finished; no replacement was started"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(true)
    }
    /// Factory runs only in the controller process. Frontend loss does not drop domain workers.
    pub fn run<C: Controller>(self, factory: impl FnOnce(&[String]) -> Result<C>) -> Result<()> {
        self.run_or_attach(factory, |_, _| false)
    }
    /// Reuse a live controller whose original arguments match this application's resource.
    /// The matcher receives its launch directory (`None` for older sessions); resource
    /// identity and path normalization belong to the application. Explicit lifecycle
    /// commands bypass matching. A stale socket is never selected for reattachment.
    pub fn run_or_attach<C: Controller>(
        self,
        factory: impl FnOnce(&[String]) -> Result<C>,
        matches: impl Fn(&[String], Option<&Path>) -> bool,
    ) -> Result<()> {
        session::private_root(&self.root)?;
        let args: Vec<String> = std::env::args().skip(1).collect();
        // Relay failures must reach the frontend over the protocol, including failures
        // before the controller's handshake. Keep the relay alive until the frontend
        // consumes the error and closes stdin, avoiding an exit/error-reader race.
        let relay = args.first().is_some_and(|a| a == "--graphical-relay");
        let result = self.run_args(args, factory, matches);
        if relay {
            if let Err(error) = result {
                tracing::error!(error = %format!("{error:#}"), "Application relay failed");
                write_message(
                    &ServerMessage::Error {
                        message: format!("{error:#}"),
                    },
                    &mut std::io::stdout().lock(),
                )?;
                std::io::stdout().flush()?;
                let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
                return Ok(());
            }
        }
        result
    }
    fn run_args<C: Controller>(
        self,
        args: Vec<String>,
        factory: impl FnOnce(&[String]) -> Result<C>,
        matches: impl Fn(&[String], Option<&Path>) -> bool,
    ) -> Result<()> {
        if args.first().is_some_and(|a| a == "--sessions") {
            for name in session::list(&self.root)? {
                println!("{name}");
            }
            return Ok(());
        }
        if args.first().is_some_and(|a| a == "--attach") {
            let name = args.get(1).context("--attach SESSION")?.clone();
            ensure!(
                session::socket_path(&self.root, &name)?.exists(),
                "Session is no longer running"
            );
            return self.launch(name, true);
        }
        let internal = matches!(
            args.first().map(String::as_str),
            Some("--serve" | "--graphical-relay")
        );
        let name = if internal {
            args.get(1).context("Missing application session")?.clone()
        } else {
            format!("{}-{}", self.name, std::process::id())
        };
        let socket = session::socket_path(&self.root, &name)?;
        let context = self.root.join(format!("{name}.args.json"));
        let failure = self.root.join(format!("{name}.startup-error"));
        if internal {
            if args[0] == "--graphical-relay" && !args.iter().any(|a| a == "--attach-only") {
                let directory = args
                    .iter()
                    .position(|a| a == "--directory")
                    .and_then(|i| args.get(i + 1));
                if !context.exists() {
                    let domain =
                        directory.map_or_else(Vec::new, |d| vec!["--directory".into(), d.clone()]);
                    let bytes = serde_json::to_vec(&Arguments::Context {
                        args: domain,
                        directory: std::env::current_dir()?,
                    })?;
                    crate::fs::write_private(&context, &bytes)?;
                } else if directory.is_some() {
                    let existing = Arguments::read(&context)?;
                    ensure!(
                        matches(existing.domain().0, existing.domain().1),
                        "This session belongs to another resource; choose another session name"
                    );
                }
            }
            if args[0] == "--serve" {
                struct Cleanup(PathBuf);
                impl Drop for Cleanup {
                    fn drop(&mut self) {
                        let _ = std::fs::remove_file(&self.0);
                    }
                }
                let _cleanup = Cleanup(context);
                let result = (|| {
                    let domain = Arguments::read(&_cleanup.0)?;
                    session::serve(&self.root, &name, factory(domain.domain().0)?)
                })();
                if let Err(error) = &result {
                    tracing::error!(error = %format!("{error:#}"), "Application controller failed");
                    crate::fs::write_private(&failure, format!("{error:#}").as_bytes())?;
                }
                return result;
            }
            let socket = self.ensure_session(&name, args.iter().any(|a| a == "--attach-only"))?;
            return session::relay(&socket);
        }
        if let Some(existing) = self.matching_session(matches)? {
            tracing::info!(session = %existing, "Reattaching to existing application controller");
            return self.launch(existing, true);
        }
        ensure!(args.len() <= 1024, "Too many application arguments");
        let bytes = serde_json::to_vec(&Arguments::Context {
            args,
            directory: std::env::current_dir()?,
        })?;
        ensure!(
            bytes.len() <= 1_048_576,
            "Application arguments exceed limits"
        );
        crate::fs::write_private(&context, &bytes)?;
        let result = self.launch(name, false);
        if !socket.exists() {
            let _ = std::fs::remove_file(context);
        }
        result
    }
    fn matching_session(
        &self,
        matches: impl Fn(&[String], Option<&Path>) -> bool,
    ) -> Result<Option<String>> {
        for name in session::list(&self.root)? {
            let Ok(domain) = Arguments::read(&self.root.join(format!("{name}.args.json"))) else {
                continue;
            };
            let (args, directory) = domain.domain();
            if matches(args, directory)
                && std::os::unix::net::UnixStream::connect(session::socket_path(&self.root, &name)?)
                    .is_ok()
            {
                return Ok(Some(name));
            }
        }
        Ok(None)
    }
    fn ensure_session(&self, name: &str, attach_only: bool) -> Result<PathBuf> {
        let socket = session::socket_path(&self.root, name)?;
        let context = self.root.join(format!("{name}.args.json"));
        let failure = self.root.join(format!("{name}.startup-error"));
        if !socket.exists() {
            ensure!(!attach_only, "Session is no longer running");
            let _ = std::fs::remove_file(&failure);
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
                if let Some(status) = child.try_wait()? {
                    let message = std::fs::read_to_string(&failure).unwrap_or_else(|_| {
                        format!("Application controller exited with {status} before starting")
                    });
                    let _ = std::fs::remove_file(&failure);
                    anyhow::bail!("Could not start application: {message}");
                }
                if start.elapsed() >= Duration::from_secs(10) {
                    // Do not leave an invisible controller holding resource locks.
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&context);
                    anyhow::bail!("Application controller did not start within 10 seconds");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(socket)
    }
    fn launch(&self, name: String, attach_only: bool) -> Result<()> {
        if let Some(bridge) = super::terminal_bridge::probe()? {
            return bridge.relay(&self.ensure_session(&name, attach_only)?);
        }
        client::run(Launch {
            executable: std::env::current_exe()?.to_string_lossy().into_owned(),
            host: None,
            ssh_config: None,
            session: name,
            directory: None,
            attach_only,
            play: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn close_waits_for_resource_cleanup_and_does_not_touch_other_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let app = Application::new("test")
            .unwrap()
            .with_root(directory.path().join("runtime"));
        session::private_root(&app.root).unwrap();
        let socket = session::socket_path(&app.root, "retained").unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let context = app.root.join("retained.args.json");
        crate::fs::write_private(&context, br#"["--project","/tmp/demo"]"#).unwrap();
        let quit = Input::Key {
            code: "char:q".into(),
            modifiers: 0,
        };
        assert!(!app
            .close_matching(
                |_, _| false,
                std::slice::from_ref(&quit),
                Duration::from_secs(2)
            )
            .unwrap());
        let worker = std::thread::spawn(move || {
            // Matching probes liveness without taking over the frontend.
            drop(listener.accept().unwrap());
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            assert!(matches!(
                read_message::<ClientMessage>(&mut reader).unwrap(),
                Some(ClientMessage::Hello { .. })
            ));
            assert!(
                matches!(read_message::<ClientMessage>(&mut reader).unwrap(), Some(ClientMessage::Input { input: Input::Key { code, .. }, .. }) if code == "char:q")
            );
            write_message(&ServerMessage::Closed, &mut stream).unwrap();
            // The domain lock and context can outlive the Closed notification.
            std::thread::sleep(Duration::from_millis(60));
            std::fs::remove_file(socket).unwrap();
            std::fs::remove_file(context).unwrap();
        });
        let start = Instant::now();
        assert!(app
            .close_matching(
                |args, _| args == ["--project", "/tmp/demo"],
                &[quit],
                Duration::from_secs(2)
            )
            .unwrap());
        assert!(start.elapsed() >= Duration::from_millis(60));
        worker.join().unwrap();
    }

    #[test]
    fn close_timeout_preserves_a_controller_that_has_not_finished() {
        let directory = tempfile::tempdir().unwrap();
        let app = Application::new("test")
            .unwrap()
            .with_root(directory.path().join("runtime"));
        session::private_root(&app.root).unwrap();
        let socket = session::socket_path(&app.root, "retained").unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let context = app.root.join("retained.args.json");
        crate::fs::write_private(&context, br#"["--project","/tmp/demo"]"#).unwrap();
        let result = app.close_matching(
            |_, _| true,
            &[Input::Key {
                code: "char:q".into(),
                modifiers: 0,
            }],
            Duration::from_millis(30),
        );
        assert!(result.is_err());
        assert!(context.exists());
        assert!(UnixStream::connect(&socket).is_ok());
        drop(listener);
    }

    #[test]
    fn reuse_matches_legacy_and_current_arguments_but_ignores_stale_sockets() {
        let directory = tempfile::tempdir().unwrap();
        let app = Application::new("test")
            .unwrap()
            .with_root(directory.path().join("runtime"));
        session::private_root(&app.root).unwrap();
        let path = session::socket_path(&app.root, "retained").unwrap();
        let listener = UnixListener::bind(path).unwrap();
        let context = app.root.join("retained.args.json");
        crate::fs::write_private(&context, br#"["--project","/tmp/demo"]"#).unwrap();
        let found = app
            .matching_session(|args, cwd| args == ["--project", "/tmp/demo"] && cwd.is_none())
            .unwrap();
        assert_eq!(found.as_deref(), Some("retained"));
        let args = Arguments::Context {
            args: vec!["--project".into(), "demo".into()],
            directory: PathBuf::from("/original"),
        };
        crate::fs::write_private(&context, &serde_json::to_vec(&args).unwrap()).unwrap();
        let matching = |args: &[String], cwd: Option<&Path>| {
            args == ["--project", "demo"] && cwd == Some(Path::new("/original"))
        };
        assert_eq!(
            app.matching_session(matching).unwrap().as_deref(),
            Some("retained")
        );
        assert!(app.matching_session(|_, _| false).unwrap().is_none());
        drop(listener);
        assert!(app.matching_session(matching).unwrap().is_none());
    }
}
