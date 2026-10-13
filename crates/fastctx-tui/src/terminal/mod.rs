//! The terminal while the TUI has it: raw input, the alternate screen,
//! pastes bracketed, the cursor hidden unless a page places it. However the
//! TUI ends, a panic included, the terminal is put back as it was.

mod colors;
mod notification;
#[cfg(windows)]
mod windows;

use std::io::{self, IsTerminal, Stdout, stdout};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub use crate::input::Focus;
pub(crate) use colors::depth;
pub use notification::NotificationProtocol;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

/// The terminal, held; dropping it gives the terminal back.
pub struct Screen {
    pub terminal: Term,
    notification_protocol: Option<NotificationProtocol>,
}

impl Screen {
    pub fn open() -> io::Result<Self> {
        require_interactive()?;
        let notification_protocol = notification::detect();
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
            Ok(terminal) => Ok(Self { terminal, notification_protocol }),
            Err(error) => {
                restore();
                Err(error)
            }
        }
    }

    /// The latest terminal focus report, or `Unknown` until one arrives.
    /// Updated by the sole input reader, even before the UI handles its events.
    pub fn focus(&self) -> Focus {
        crate::input::focus()
    }

    /// The identified transport. Unknown terminals, multiplexers, native
    /// conhost and Windows Terminal without a verifiable version return `None`.
    pub fn notification_protocol(&self) -> Option<NotificationProtocol> {
        self.notification_protocol
    }

    /// Submits one notification only when the terminal is known to be unfocused.
    /// Call once for a completed, failed or stopped turn in the viewed session;
    /// the caller chooses the title and body and owns turn deduplication.
    ///
    /// Returns `true` when written and flushed, not when displayed by the OS.
    /// Unknown focus/support and foreground calls return `false` and are never
    /// queued. Control characters become spaces; OSC 9/777 replace semicolons
    /// with commas. No bell, capability query, retry or input reply is requested.
    /// Use on the UI thread between draws, after [`crate::input::start`].
    pub fn notify(&mut self, title: &str, body: &str) -> io::Result<bool> {
        notification::notify(&mut stdout().lock(), self.notification_protocol, crate::input::focus, title, body)
    }
}

impl Screen {
    /// Hands the terminal to another program for as long as `run` takes,
    /// and takes it back: meanwhile the screen the user had shows and every
    /// key goes to that program alone, as the input reader is stopped; it
    /// starts again, sending to `input`, and the whole screen is drawn anew.
    pub fn hand_over<T>(
        &mut self,
        input: &tokio::sync::mpsc::UnboundedSender<crate::input::Input>,
        run: impl FnOnce() -> T,
    ) -> io::Result<T> {
        let _ = self.terminal.show_cursor();
        restore();
        let returned = run();
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        crate::input::start(input.clone())?;
        self.terminal.clear()?;
        Ok(returned)
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
    let input = crate::input::restore();
    let _ = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen, crossterm::cursor::Show);
    let _ = disable_raw_mode();
    // Said once the screen the user had is back, where it can be read.
    if let Err(error) = input {
        eprintln!("fastctx: giving the terminal's input back: {error}");
    }
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
