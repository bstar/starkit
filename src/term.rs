//! Terminal setup and teardown.
//!
//! The panic hook is not optional. A TUI that panics without restoring the
//! terminal leaves the user with no echo, no cursor and a scrambled screen, and
//! they have to blind-type `reset`. Restoring first, then printing the panic, is
//! the difference between a bug report and a bad afternoon.

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
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

pub fn init() -> Result<Tui> {
    install_panic_hook();
    enable_raw_mode()?;
    ENTERED.store(true, Ordering::SeqCst);
    let mut out = io::stdout();
    execute!(
        out,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste,
        crossterm::cursor::Hide
    )?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;
    term.clear()?;
    Ok(term)
}

pub fn restore() -> Result<()> {
    // Cleared first, so that an application which drops back to the ordinary
    // screen -- to run an external viewer, or an editor -- may probe again,
    // and so that a failure below does not leave the flag saying the keyboard
    // is still owned.
    ENTERED.store(false, Ordering::SeqCst);
    let mut out = io::stdout();
    execute!(
        out,
        crossterm::cursor::Show,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;
    Ok(())
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Restore before reporting, or the report itself is unreadable.
        let _ = restore();
        previous(info);
    }));
}
