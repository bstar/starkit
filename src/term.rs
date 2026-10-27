//! Terminal setup and teardown.
//!
//! The panic hook is not optional. A TUI that panics without restoring the
//! terminal leaves the user with no echo, no cursor and a scrambled screen, and
//! they have to blind-type `reset`. Restoring first, then printing the panic, is
//! the difference between a bug report and a bad afternoon.
//!
//! ## Nothing here asks the terminal a question
//!
//! Taking the screen writes escapes and reads nothing back. That is a rule
//! rather than an accident: `Terminal::clear` asks the backend where the cursor
//! is so it can put it back afterwards, and asking means writing `\e[6n` and
//! blocking on the reply. A terminal with nobody behind it -- a tmux pane with
//! no attached client, a bare pty, some CI harnesses -- never answers, and the
//! application hangs before its first frame with no message in it. STAR/CORD
//! would only start under `| cat`, which redirects the output and makes the
//! query a no-op, until this stopped happening.
//!
//! So the screen is cleared with an escape of our own and the terminal is
//! built afterwards. A fresh `Terminal`'s back buffer is already blank, which
//! is exactly what is on the screen after the clear, so the first frame draws
//! the cells that are not blank and nothing else -- which is what
//! `Terminal::clear` was being called for.
//!
//! The same rule is why [`crate::graphics::Graphics::probe`] runs before this:
//! it *does* ask questions, and it has to ask them while the answers still
//! arrive on stdin as replies rather than as keystrokes.

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Whether the application currently owns the keyboard.
///
/// Read by `graphics::Graphics::probe`, which writes a capability query to the
/// terminal and reads the reply off stdin. Once raw mode is on, that reply
/// arrives interleaved with the user's keystrokes -- the probe reports whatever
/// they typed as the terminal's answer, and the keys they meant to press are
/// eaten. The ordering cannot be expressed in a type across a crate boundary,
/// so it is recorded here instead and asserted there.
static ENTERED: AtomicBool = AtomicBool::new(false);

/// Has the terminal been taken over since the last [`restore`]?
pub fn entered() -> bool {
    ENTERED.load(Ordering::SeqCst)
}

/// Take the screen: raw mode, the alternate screen, mouse capture, bracketed
/// paste and focus reporting, cleared and ready to draw on.
///
/// Focus reporting is on because both applications have something to stop
/// doing when the window is not in front -- acknowledging messages somebody
/// else is reading, animating a GIF nobody can see. A terminal that does not
/// support it simply never sends [`crossterm::event::Event::FocusGained`] or
/// `FocusLost`, so an application that ignores both is no worse off than
/// before.
pub fn init() -> Result<Tui> {
    install_panic_hook();
    enable_raw_mode()?;
    ENTERED.store(true, Ordering::SeqCst);
    let result = (|| {
        let mut out = io::stdout();
        enter(&mut out)?;
        // Construct after clearing; never query the cursor position.
        Ok(Terminal::new(CrosstermBackend::new(out))?)
    })();
    if result.is_err() {
        let _ = restore();
    }
    result
}

/// A terminal owner that restores modes on ordinary early returns as well as
/// panics. Probe capabilities first. Dereferences to the standard ratatui Tui.
/// Use one owner per process; external viewers should explicitly restore/reinit.
pub struct Session {
    terminal: Tui,
}
impl Session {
    pub fn new() -> Result<Self> {
        Ok(Self { terminal: init()? })
    }
    pub fn finish(self) -> Result<()> {
        restore()
    }
}
impl std::ops::Deref for Session {
    type Target = Tui;
    fn deref(&self) -> &Tui {
        &self.terminal
    }
}
impl std::ops::DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Tui {
        &mut self.terminal
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if entered() {
            let _ = restore();
        }
    }
}

pub fn restore() -> Result<()> {
    // Cleared first, so that an application which drops back to the ordinary
    // screen -- to run an external viewer, or an editor -- may probe again,
    // and so that a failure below does not leave the flag saying the keyboard
    // is still owned.
    ENTERED.store(false, Ordering::SeqCst);
    let mut out = io::stdout();
    restore_inner(&mut out, || {
        // Disable reporting before draining bytes already in flight. Keep raw
        // mode until the queue is empty so mouse reports cannot reach the shell.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(30);
        while std::time::Instant::now() < deadline {
            match crate::crossterm::event::poll(std::time::Duration::from_millis(2)) {
                Ok(true) => {
                    if crate::crossterm::event::read().is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        // Polling drains crossterm's parser; also discard a partial SGR report
        // still in the kernel before switching back to the shell's line mode.
        #[cfg(all(unix, feature = "terminal-graphics"))]
        let _ = rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush);
        disable_raw_mode()
    })?;
    Ok(())
}

fn restore_inner(out: &mut impl Write, raw: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    let screen = leave(out);
    let keyboard = raw();
    screen.and(keyboard)
}

/// Everything [`init`] writes to the terminal, and nothing it reads.
///
/// Split out from `init` so that a test can run it against a vector and read
/// the bytes back: what matters about this sequence is as much what is not in
/// it -- `\e[6n` -- as what is.
fn enter(out: &mut impl Write) -> io::Result<()> {
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        ),
        Hide,
        Clear(ClearType::All),
        MoveTo(0, 0),
    )
}

/// The inverse of [`enter`], in the opposite order.
fn leave(out: &mut impl Write) -> io::Result<()> {
    #[cfg(feature = "terminal-graphics")]
    // Cancel any in-flight request and unregister the drop target before raw
    // input is released. Cancelling alone leaves Kitty reporting drag motion
    // to the next shell. This also covers early errors and panic restoration.
    out.write_all(b"\x1b]72;t=r:o=0:i=1\x1b\\\x1b]72;t=A:i=1\x1b\\")?;
    // The graphical frontend may have enabled SGR pixel mouse coordinates.
    // Include this in panic restoration as well as its normal mode guard.
    out.write_all(b"\x1b[?1016l")?;
    execute!(
        out,
        Show,
        PopKeyboardEnhancementFlags,
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
    )
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Restore before reporting, or the report itself is unreadable.
        let _ = restore();
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_screen_restoration_still_releases_raw_keyboard_mode() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut released = false;
        assert!(restore_inner(&mut Broken, || {
            released = true;
            Ok(())
        })
        .is_err());
        assert!(released);
    }
    fn written(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).expect("a vector cannot fail to be written to");
        String::from_utf8(out).expect("escape sequences are ASCII")
    }

    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn restoration_unregisters_drag_reporting_before_leaving_the_screen() {
        let off = written(leave);
        let cancel = off.find("\u{1b}]72;t=r:o=0:i=1\u{1b}\\").unwrap();
        let unregister = off.find("\u{1b}]72;t=A:i=1\u{1b}\\").unwrap();
        let screen = off.find("\u{1b}[?1049l").unwrap();
        assert!(cancel < unregister && unregister < screen);
    }

    /// The bug this whole arrangement exists for. `\e[6n` is a question, and a
    /// terminal with nobody behind it does not answer questions; the
    /// application then waits for the reply forever, before it has drawn
    /// anything at all, with nothing on screen to say why.
    #[test]
    fn taking_the_screen_never_asks_where_the_cursor_is() {
        let seq = written(enter);
        assert!(
            !seq.contains("\u{1b}[6n"),
            "init asks for the cursor position: {seq:?}"
        );
        assert!(
            seq.contains("\u{1b}[2J"),
            "the screen is cleared by us, or not at all: {seq:?}"
        );
        assert!(!written(leave).contains("\u{1b}[6n"));
    }

    /// Every mode turned on is turned off again. A left-behind mouse capture
    /// is a terminal that prints garbage on every click after the application
    /// has exited.
    #[test]
    fn every_mode_is_given_back() {
        let on = written(enter);
        let off = written(leave);
        for (name, mode) in [
            ("the alternate screen", "1049"),
            ("mouse capture", "1006"),
            ("bracketed paste", "2004"),
            ("focus reporting", "1004"),
        ] {
            assert!(on.contains(&format!("\u{1b}[?{mode}h")), "{name} is not on");
            assert!(
                off.contains(&format!("\u{1b}[?{mode}l")),
                "{name} is left on"
            );
        }
        assert!(off.contains("\u{1b}[?1016l"), "pixel mouse mode is left on");
        assert!(on.contains("\u{1b}[>5u"));
        assert!(off.contains("\u{1b}[<1u"));
        assert!(on.contains("\u{1b}[?25l"), "the cursor is left visible");
        assert!(off.contains("\u{1b}[?25h"), "the cursor is left hidden");
    }
}
