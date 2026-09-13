//! Terminal setup and teardown.
//!
//! The panic hook is not optional. A TUI that panics without restoring the
//! terminal leaves the user with no echo, no cursor and a scrambled screen, and
//! they have to blind-type `reset`. Restoring first, then printing the panic, is
//! the difference between a bug report and a bad afternoon.

use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn init() -> Result<Tui> {
    install_panic_hook();
    enable_raw_mode()?;
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
