//! The terminal while the TUI has it: raw input, the alternate screen,
//! pastes bracketed, the cursor hidden unless a page places it. However the
//! TUI ends, a panic included, the terminal is put back as it was.

mod colors;
#[cfg(windows)]
mod windows;

use std::io::{self, IsTerminal, Stdout, stdout};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub(crate) use colors::depth;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

/// The terminal, held; dropping it gives the terminal back.
pub struct Screen {
    pub terminal: Term,
}

impl Screen {
    pub fn open() -> io::Result<Self> {
        require_interactive()?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Put the terminal back first, so the panic is printed on the
            // screen the user had.
            restore();
            previous(info);
        }));
        enable_raw_mode()?;
        if let Err(error) = execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste) {
            restore();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(stdout())) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                restore();
                Err(error)
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        restore();
    }
}

/// Gives the terminal back as it was; safe to call more than once.
fn restore() {
    crate::input::restore();
    let _ = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen, crossterm::cursor::Show);
    let _ = disable_raw_mode();
}

/// Checks both streams before raw mode, alternate screen, or an input
/// reader is started. `NO_COLOR` does not prohibit an interactive TUI.
///
/// Rust's terminal check also recognizes MSYS/Cygwin PTY pipes on Windows;
/// a `TERM` variable alone never makes a redirected stream interactive.
pub fn require_interactive() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "the TUI needs terminal input and output"));
    }
    if colors::variable("TERM") == "dumb" {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "TERM=dumb cannot run a full-screen TUI"));
    }
    Ok(())
}
