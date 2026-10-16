//! OpenSSH adapter for desktop clients. The HTTP/session contract is also usable
//! by an embedded SSH implementation on tvOS; no terminal protocol is required.
use super::Session;
use anyhow::{ensure, Context, Result};
use std::{
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub host: String,
    pub config: Option<PathBuf>,
}
impl Endpoint {
    fn command(&self) -> Result<Command> {
        ensure!(
            !self.host.is_empty()
                && !self.host.starts_with('-')
                && !self.host.chars().any(char::is_control),
            "Invalid SSH address"
        );
        let mut command = Command::new("ssh");
        if let Some(config) = &self.config {
            command.arg("-F").arg(config);
        }
        Ok(command)
    }
    /// Launch an application-owned service, read its bounded JSON bootstrap,
    /// then add a loopback forward through the same authenticated SSH master.
    pub fn connect(&self, executable: &str, arguments: &[String]) -> Result<Connection> {
        self.open(executable, arguments, None)
    }
    /// Reuse an authenticated desktop connection without owning its lifetime.
    pub fn connect_via_master(
        &self,
        control: &std::path::Path,
        executable: &str,
        arguments: &[String],
    ) -> Result<Connection> {
        ensure!(
            control.is_absolute() && control.exists(),
            "SSH control socket is unavailable"
        );
        self.open(executable, arguments, Some(control))
    }
    fn open(
        &self,
        executable: &str,
        arguments: &[String],
        master: Option<&std::path::Path>,
    ) -> Result<Connection> {
        ensure!(
            !executable.is_empty()
                && !executable.contains('\0')
                && arguments.iter().all(|a| !a.contains('\0')),
            "Invalid remote command"
        );
        let directory = tempfile::Builder::new()
            .prefix("star-movie-ssh-")
            .tempdir_in("/tmp")?;
        let control =
            master.map_or_else(|| directory.path().join("s"), std::path::Path::to_path_buf);
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let remote = std::iter::once(executable)
            .chain(arguments.iter().map(String::as_str))
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = self.command()?;
        command.args(["-T", "-S"]).arg(&control);
        if master.is_none() {
            command.args(["-M", "-o", "ControlPersist=no"]);
        }
        let child = command
            .args([
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "--",
                &self.host,
                &remote,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut connection = Connection {
            child,
            session: Session {
                version: 1,
                port: 0,
                token: String::new(),
            },
            _directory: directory,
            forward: None,
        };
        let stdout = connection
            .child
            .stdout
            .take()
            .context("No SSH service output")?;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout.take(16 * 1024))
                .read_line(&mut line)
                .map(|_| line);
            let _ = send.send(result);
        });
        let line = receive
            .recv_timeout(Duration::from_secs(60))
            .context("SSH movie service bootstrap timed out")??;
        ensure!(
            line.ends_with('\n') && line.len() <= 16 * 1024,
            "Incomplete or oversized movie service bootstrap"
        );
        let mut session: Session =
            serde_json::from_str(&line).context("Invalid movie service bootstrap")?;
        session.validate()?;
        let reservation = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        let forwarding = format!("127.0.0.1:{port}:127.0.0.1:{}", session.port);
        let status = self
            .command()?
            .arg("-S")
            .arg(&control)
            .args([
                "-O",
                "forward",
                "-o",
                "ExitOnForwardFailure=yes",
                "-L",
                &forwarding,
                "--",
                &self.host,
            ])
            .status()?;
        ensure!(status.success(), "SSH could not establish the movie tunnel");
        session.port = port;
        connection.session = session;
        connection.forward = Some((self.clone(), control, forwarding));
        Ok(connection)
    }
}
pub struct Connection {
    child: Child,
    pub session: Session,
    _directory: tempfile::TempDir,
    forward: Option<(Endpoint, PathBuf, String)>,
}
impl Connection {
    pub fn is_running(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some((endpoint, control, forward)) = self.forward.take() {
            if let Ok(mut command) = endpoint.command() {
                let _ = command
                    .arg("-S")
                    .arg(control)
                    .args(["-O", "cancel", "-L", &forward, "--", &endpoint.host])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_addresses_cannot_become_options_or_lines() {
        for host in ["", "-oProxyCommand=bad", "host\nother"] {
            assert!(Endpoint {
                host: host.into(),
                config: None
            }
            .command()
            .is_err());
        }
        assert!(Endpoint {
            host: "person@host".into(),
            config: None
        }
        .command()
        .is_ok());
    }
}
