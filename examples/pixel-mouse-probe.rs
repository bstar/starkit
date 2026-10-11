use starkit::terminal_graphics::{
    client::{self, Launch},
    protocol::*,
    session::{self, Controller},
};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Default)]
struct Probe(bool);
impl Controller for Probe {
    fn tick(&mut self) {}
    fn closed(&self) -> bool {
        self.0
    }
    fn shutdown(&mut self) {}
    fn input(&mut self, input: Input) {
        if matches!(&input, Input::Pointer { .. }) {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::var("PIXEL_MOUSE_RECORD").unwrap())
                .unwrap();
            writeln!(f, "{}", serde_json::to_string(&input).unwrap()).unwrap();
        }
        if matches!(&input, Input::Key{code,..} if code=="char:q") {
            self.0 = true;
        }
    }
    fn scene(&mut self, v: Viewport) -> Scene {
        let b = starkit::ratatui::buffer::Buffer::empty(starkit::ratatui::layout::Rect::new(
            0, 0, v.columns, v.rows,
        ));
        Scene::from_buffer(&b, v, 1)
    }
}
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(std::env::var("PIXEL_MOUSE_ROOT")?);
    session::private_root(&root)?;
    if args.get(1).is_some_and(|s| s == "--serve") {
        return session::serve(&root, "probe", Probe::default());
    }
    if args.get(1).is_some_and(|s| s == "--graphical-relay") {
        let socket = session::socket_path(&root, "probe")?;
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()?;
        let start = Instant::now();
        while !socket.exists() {
            anyhow::ensure!(
                child.try_wait()?.is_none() && start.elapsed() < Duration::from_secs(5),
                "startup failed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        return session::relay(&socket);
    }
    client::run(Launch {
        executable: std::env::current_exe()?.to_string_lossy().into(),
        host: None,
        ssh_config: None,
        session: "probe".into(),
        directory: None,
        attach_only: false,
        play: None,
    })
}
