//! The terminal while the TUI has it: raw input, the alternate screen,
//! pastes bracketed, the cursor hidden unless a page places it. However the
//! TUI ends, a panic included, the terminal is put back as it was.

use std::io::{Stdout, stdout};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

/// The terminal, held; dropping it gives the terminal back.
pub struct Screen {
    pub terminal: Term,
}

impl Screen {
    pub fn open() -> std::io::Result<Self> {
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
