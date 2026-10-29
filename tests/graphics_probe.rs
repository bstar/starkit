//! Capability timing and terminal ownership against actual, isolated PTYs.
#![cfg(all(unix, feature = "image"))]
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const REPLY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b[?1;2c\x1b[6;18;9t\x1b[0n";
struct Running {
    child: Child,
    master: Option<File>,
    output: Vec<u8>,
}
impl Drop for Running {
    fn drop(&mut self) {
        drop(self.master.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Running {
    fn until(&mut self, marker: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self
                .output
                .windows(marker.len())
                .any(|bytes| bytes == marker)
            {
                return;
            }
            let mut buffer = [0; 4096];
            match self.master.as_mut().unwrap().read(&mut buffer) {
                Ok(n) => self.output.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
            if self
                .output
                .windows(marker.len())
                .any(|bytes| bytes == marker)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "PTY did not produce {:?}: {}",
            String::from_utf8_lossy(marker),
            String::from_utf8_lossy(&self.output)
        );
    }
    fn send(&mut self, bytes: &[u8]) {
        self.master.as_mut().unwrap().write_all(bytes).unwrap();
    }
    fn finish(mut self) {
        self.until(b"KEY_OK");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("PTY child did not exit");
    }
}
fn child(case: &str, timeout_ms: u64) -> Running {
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: 30,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes both descriptors and borrows a valid size.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(size),
            )
        },
        0
    );
    // SAFETY: transfer the two newly owned descriptors exactly once.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    // SAFETY: fcntl receives live descriptors and no pointers.
    unsafe {
        assert_eq!(
            libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC),
            0
        );
        assert_eq!(
            libc::fcntl(slave.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC),
            0
        );
        assert_eq!(
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK),
            0
        );
    }
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "probe_child", "--nocapture"])
        .env("KIT_PROBE_CHILD", case)
        .env("STAR_GRAPHICS_PROBE_TIMEOUT_MS", timeout_ms.to_string())
        .env("TERM", "xterm-kitty")
        .env_remove("TERM_PROGRAM")
        .env_remove("TMUX")
        .env_remove("STY")
        .env_remove("WEZTERM_EXECUTABLE")
        .env_remove("KONSOLE_VERSION")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    if case == "tmux" {
        cmd.env("TMUX", "fixture");
    }
    if case == "iterm" {
        cmd.env("TERM_PROGRAM", "iTerm.app");
    }
    // SAFETY: only async-signal-safe syscalls run before exec; fd 0 is the PTY.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().unwrap();
    Running {
        child,
        master: Some(master),
        output: vec![],
    }
}
fn probe_case(case: &str, reply: &[u8], delay: Duration, timeout_ms: u64) {
    let mut run = child(case, timeout_ms);
    run.until(b"\x1b[5n");
    std::thread::sleep(delay);
    if !reply.is_empty() {
        run.send(reply);
    }
    run.until(b"READY_FOR_KEY");
    if case != "trailing" {
        run.send(b"q");
    }
    run.finish();
}
#[test]
fn delayed_ssh_reply_and_trailing_key_are_preserved() {
    probe_case("delayed", REPLY, Duration::from_millis(650), 1500);
    let mut reply = REPLY.to_vec();
    reply.push(b'q');
    probe_case("trailing", &reply, Duration::ZERO, 1500);
}
#[test]
fn silent_and_incomplete_terminals_return_without_a_reader() {
    probe_case("silent", b"", Duration::ZERO, 100);
    probe_case("partial", b"\x1b_Gi=31;", Duration::ZERO, 100);
}
#[test]
fn late_reply_cannot_restore_cooked_mode_or_steal_the_next_key() {
    probe_case("late", REPLY, Duration::from_millis(350), 100);
}
#[test]
fn raw_and_custom_terminal_modes_are_restored_exactly() {
    probe_case("raw", REPLY, Duration::ZERO, 1000);
    probe_case("custom", REPLY, Duration::ZERO, 1000);
}
#[test]
fn fragmented_reply_and_non_kitty_protocols_work() {
    let mut run = child("fragmented", 1500);
    run.until(b"\x1b[5n");
    for byte in REPLY {
        run.send(&[*byte]);
        std::thread::sleep(Duration::from_millis(2));
    }
    run.until(b"READY_FOR_KEY");
    run.send(b"q");
    run.finish();
    probe_case(
        "sixel",
        b"\x1b[?62;4c\x1b[6;18;9t\x1b[0n",
        Duration::ZERO,
        1000,
    );
    probe_case("iterm", b"\x1b[6;18;9t\x1b[0n", Duration::ZERO, 1000);
}
#[test]
fn guessed_font_size_is_not_advertised_as_measured() {
    probe_case(
        "unmeasured",
        b"\x1b_Gi=31;OK\x1b\\\x1b[0n",
        Duration::ZERO,
        1000,
    );
}
#[test]
fn automatic_multiplexer_fallback_does_not_query_or_consume_input() {
    let mut run = child("tmux", 1000);
    run.until(b"READY_FOR_KEY");
    assert!(!run.output.windows(4).any(|bytes| bytes == b"\x1b[5n"));
    run.send(b"q");
    run.finish();
}
fn attributes() -> (u64, u64, u64, u64, Vec<u8>) {
    let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr initializes term on success, checked before assume_init.
    assert_eq!(unsafe { libc::tcgetattr(0, term.as_mut_ptr()) }, 0);
    let term = unsafe { term.assume_init() };
    // Darwin's TIOCSETA sets PENDIN when returning to canonical mode and
    // preserves it even if the supplied termios clears it. This is kernel
    // queue state, cleared by ttypend on the next input/read, not a changed
    // application setting. Compare every other flag and all control bytes;
    // the subsequent key assertion also verifies the pending input survives.
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/tty.c
    #[cfg(target_os = "macos")]
    let local = term.c_lflag & !libc::PENDIN;
    #[cfg(not(target_os = "macos"))]
    let local = term.c_lflag;
    (
        term.c_iflag as u64,
        term.c_oflag as u64,
        term.c_cflag as u64,
        local as u64,
        term.c_cc.to_vec(),
    )
}
#[test]
fn probe_child() {
    use starkit::crossterm::{
        event::{self, Event, KeyCode},
        terminal,
    };
    let Ok(case) = std::env::var("KIT_PROBE_CHILD") else {
        return;
    };
    if case == "raw" {
        terminal::enable_raw_mode().unwrap();
    }
    if case == "custom" {
        let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: operate only on this child's private PTY and initialized attrs.
        unsafe {
            assert_eq!(libc::tcgetattr(0, term.as_mut_ptr()), 0);
            let mut term = term.assume_init();
            term.c_lflag &= !libc::ECHO;
            assert_eq!(libc::tcsetattr(0, libc::TCSANOW, &term), 0);
        }
    }
    let original = attributes();
    let was_raw = terminal::is_raw_mode_enabled().unwrap();
    // SAFETY: inspect live standard descriptors; neither call mutates them.
    let flags = unsafe { [libc::fcntl(0, libc::F_GETFL), libc::fcntl(1, libc::F_GETFL)] };
    #[cfg(target_os = "linux")]
    let threads = std::fs::read_dir("/proc/self/task").unwrap().count();
    let began = Instant::now();
    let graphics = starkit::graphics::Graphics::probe_if_tty(starkit::graphics::Mode::Auto);
    assert!(began.elapsed() < Duration::from_secs(3));
    assert_eq!(
        attributes(),
        original,
        "Capability query changed terminal attributes"
    );
    assert_eq!(terminal::is_raw_mode_enabled().unwrap(), was_raw);
    assert_eq!(flags, unsafe {
        [libc::fcntl(0, libc::F_GETFL), libc::fcntl(1, libc::F_GETFL)]
    });
    #[cfg(target_os = "linux")]
    assert_eq!(
        std::fs::read_dir("/proc/self/task").unwrap().count(),
        threads,
        "Probe left a reader thread alive"
    );
    let fallback = matches!(case.as_str(), "silent" | "partial" | "late" | "tmux");
    assert_eq!(
        graphics.name(),
        if fallback {
            "half blocks"
        } else if case == "sixel" {
            "sixel"
        } else if case == "iterm" {
            "iterm2"
        } else {
            "kitty"
        }
    );
    assert_eq!(
        graphics.cell_size(),
        if fallback || case == "unmeasured" {
            None
        } else {
            Some((9, 18))
        }
    );
    #[cfg(feature = "terminal-graphics")]
    {
        use starkit::terminal_graphics::capabilities::{Capabilities, PixelGeometry};
        assert_eq!(
            Capabilities::detected(&graphics).pixel_geometry,
            if graphics.cell_size().is_some() {
                PixelGeometry::Measured
            } else {
                PixelGeometry::Estimated
            }
        );
    }
    terminal::enable_raw_mode().unwrap();
    println!("READY_FOR_KEY");
    io::stdout().flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = false;
    while Instant::now() < deadline {
        if event::poll(Duration::from_millis(50)).unwrap() {
            if let Ok(Event::Key(key)) = event::read() {
                if key.code == KeyCode::Char('q') {
                    received = true;
                    break;
                }
            }
        }
    }
    assert!(
        received,
        "A timed-out query stole subsequent keyboard input"
    );
    let modes = attributes().3;
    assert_eq!(
        modes & (libc::ICANON | libc::ECHO) as u64,
        0,
        "Late query completion restored cooked mode"
    );
    terminal::disable_raw_mode().unwrap();
    println!("KEY_OK");
    io::stdout().flush().unwrap();
}
